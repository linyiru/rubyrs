# sinatra_lite.rb — a *micro* subset of Sinatra::Base, implemented in pure
# Ruby on top of rubyrs's `_http_server` battery (ADR 0022).
#
# This is NOT the real Sinatra. It implements enough of the modular
# (`Sinatra::Base` subclass) API for the shared poc/sinatra/app.rb to run
# unmodified against BOTH this and the real gem:
#
#   * route DSL:   get / post / put / delete, with path params (":name")
#                  and splat ("*") segments  (no regex — pure segment
#                  matching, so it works in a Tier-1 build too)
#   * params:      path captures + parsed query string (URL-decoded),
#                  string keys, plus params["splat"] => Array
#   * filters:     before { ... } blocks, run in instance context
#   * helpers:     instance methods + request_body
#   * responses:   status(n), content_type(ct), headers(h),
#                  redirect(loc), halt(...)  — Sinatra's response sugar
#   * not_found:   custom 404 handler block
#   * Rack output: [status, headers, body] triplets
#   * run!:        binds the Rust HTTP front and blocks
#
# Implementation notes on rubyrs gaps worked around here (see GAPS.md):
#   * `halt`/`redirect` use a rescued exception rather than
#     `throw`/`catch` (Kernel#catch is unsupported — GAP #8).

module Sinatra
  # Minimal Rack::Request-ish wrapper. Real Sinatra exposes `request`
  # inside route blocks; apps read `request.user_agent`, `request.path`,
  # `request.request_method`, header access, etc.
  class Request
    def initialize(env)
      @env = env
    end
    def request_method; @env["REQUEST_METHOD"]; end
    def path;           @env["PATH_INFO"]; end
    def user_agent;     @env["HTTP_USER_AGENT"]; end
    def content_type;   @env["CONTENT_TYPE"]; end
    # request["Some-Header"] -> the HTTP_SOME_HEADER env value
    def [](name)
      @env["HTTP_#{name.to_s.upcase.gsub('-', '_')}"]
    end
    # request.cookies -> { "name" => "value", ... } from the Cookie header.
    def cookies
      out = {}
      raw = @env["HTTP_COOKIE"]
      return out if raw.nil? || raw.empty?
      raw.split("; ").each do |pair|
        k, v = pair.split("=", 2)
        out[k] = v || "" unless k.nil?
      end
      out
    end
  end

  # A streaming Rack body. The route's `stream { |out| ... }` block is run
  # lazily: rubyrs's `_http_server` battery invokes `call(out)` inside a
  # Fiber (ADR 0023), so each `out << chunk` becomes a chunked HTTP/1.1
  # frame flushed to the socket — true async streaming, same source as
  # real Sinatra's `stream`.
  class StreamingBody
    def initialize(blk)
      @blk = blk
    end
    def call(out)
      @blk.call(out)
      out.close
    end
  end

  class Base
    class << self
      def routes;         @routes         ||= []; end
      def filters;        @filters        ||= []; end
      def error_handlers; @error_handlers ||= []; end

      def get(path, &block);    add_route("GET", path, &block);    end
      def post(path, &block);   add_route("POST", path, &block);   end
      def put(path, &block);    add_route("PUT", path, &block);    end
      def delete(path, &block); add_route("DELETE", path, &block); end

      def add_route(verb, path, &block)
        routes << [verb, compile(path), block]
      end

      # Runs before every route (in the request instance's context).
      def before(&block)
        filters << block
      end

      # Maps an exception class raised inside a route to a handler block.
      # `error MyError do ... end`  /  `error do ... end` (StandardError).
      def error(klass = StandardError, &block)
        error_handlers << [klass, block]
      end

      # Settings store. Real Sinatra's `set`/`enable`/`disable` configure
      # behaviour (environment, sessions, …). The micro-Sinatra's
      # behaviour is fixed, so these just record values for compatibility
      # (e.g. the shared app does `set :environment, :production`).
      def settings_store; @settings_store ||= {}; end
      # Sinatra's `set :foo, val` doubles as both storage AND a
      # reflection surface — `settings.foo` returns the value
      # and `settings.respond_to?(:foo)` reports true. Real
      # Sinatra implements this by defining singleton methods
      # on the app class; we mirror that so plugins like
      # sinatra-jsonp's `settings.respond_to?(:json_pretty) &&
      # settings.json_pretty` predicate-and-read shape works.
      def set(key, value = nil)
        settings_store[key] = value
        define_singleton_method(key) { settings_store[key] } unless respond_to?(key)
        self
      end
      def enable(*keys);  keys.each { |k| set(k, true) };  self; end
      def disable(*keys); keys.each { |k| set(k, false) }; self; end

      # Custom 404 handler.
      def not_found(&block)
        @not_found = block
      end
      def not_found_handler; @not_found; end

      # `Sinatra::Base.register Module [, ...]` — the canonical
      # third-party plugin authoring entry point. Each `ext` is a
      # Module whose `self.registered(app)` hook installs filters /
      # routes / helpers onto the app. Mirrors the real Sinatra
      # gem's API surface (sinatra-cors / sinatra-flash /
      # sinatra-respond_to all use this entry). The arg-iteration
      # shape (multiple modules in one call) matches real
      # Sinatra's `register *extensions`.
      def register(*extensions)
        extensions.each do |ext|
          ext.registered(self) if ext.respond_to?(:registered)
        end
      end

      # `Sinatra::Base.helpers Module [, ...]` — mixes helper
      # modules into the app class so the modules' instance
      # methods become reachable from route blocks (which run
      # via instance_exec on a per-request dispatch instance).
      # Module form only; the block form `helpers { def ...; end }`
      # would require class_eval-with-binding, which is the
      # Tier-2 `_full_eval` boundary (ADR 0019). Plugin authors
      # who need the block form can package the helpers as a
      # Module instead — the more portable shape.
      def helpers(*modules)
        modules.each { |m| include m }
      end

      # "/say/*/to/:name" -> [[:lit,"say"],[:splat],[:lit,"to"],[:cap,"name"]]
      def compile(path)
        path.split("/").reject { |seg| seg.empty? }.map do |seg|
          if seg == "*"
            [:splat]
          elsif seg.start_with?(":")
            [:cap, seg[1..-1]]
          else
            [:lit, seg]
          end
        end
      end

      def call(env)
        new.dispatch(env)
      end

      def run!(opts = {})
        bind     = opts[:bind] || "127.0.0.1"
        port     = opts[:port] || 4567
        duration = opts[:duration] || 86_400
        app = ->(env) { call(env) }
        puts "== rubyrs micro-Sinatra serving on http://#{bind}:#{port} (Ctrl-C to stop)"
        __rubyrs_http_serve_with_app("#{bind}:#{port}", duration, app)
      end
    end

    # ---- response sugar (Sinatra-compatible) ----

    def status(code = nil)
      @status = code if code
      @status
    end

    def headers(hash = nil)
      @headers ||= { "Content-Type" => "text/html" }
      @headers.merge!(hash) if hash
      @headers
    end

    # Symbol shorthands per Sinatra docs: `content_type :json`,
    # `content_type :js`, etc. The minimal map below covers the
    # mime types that vendored third-party plugins actually
    # reach for (sinatra-jsonp uses `:js` + `:json`); the
    # passthrough else-branch keeps String args (`content_type
    # "text/csv"`) working unchanged. Real Sinatra walks a full
    # `Rack::Mime::MIME_TYPES` table; we just hardcode the
    # common subset until a fixture needs more.
    CONTENT_TYPE_SHORTHANDS = {
      json: "application/json",
      # Real Sinatra (via Rack::Mime) maps `:js` to `text/javascript`,
      # NOT `application/javascript`. RFC 9239 (Apr 2022) explicitly
      # un-deprecated `text/javascript` as the canonical JS media
      # type; Rack::Mime tracked that. Pin to `text/javascript` so
      # the same `content_type :js` lands byte-identical on both
      # runtimes.
      js:   "text/javascript",
      html: "text/html",
      txt:  "text/plain",
      xml:  "application/xml",
      csv:  "text/csv",
    }.freeze
    def content_type(type)
      headers["Content-Type"] = if type.is_a?(Symbol)
        CONTENT_TYPE_SHORTHANDS.fetch(type) do
          raise ArgumentError, "Unknown media type for #{type.inspect}"
        end
      else
        type.to_s
      end
    end

    def redirect(location, code = 302)
      # Real Sinatra expands a path to an absolute URL using the request
      # host (the Location header is absolute). Match that.
      if location.start_with?("/")
        host = @env["HTTP_HOST"] || "#{@env['SERVER_NAME']}:#{@env['SERVER_PORT']}"
        location = "http://#{host}#{location}"
      end
      headers["Location"] = location
      halt(code, "")
    end

    # halt / halt(code) / halt(body) / halt(code, body). Real Sinatra
    # implements this with `throw :halt` — and so do we, now that
    # Kernel#catch/#throw exist (GAP #8, fixed). `dispatch` wraps the
    # request in `catch(:halt)`.
    def halt(*args)
      code = @status || 200
      body = ""
      if args.length == 1
        if args[0].is_a?(Integer)
          code = args[0]
        else
          body = args[0]
        end
      elsif args.length >= 2
        code = args[0]
        body = args[1]
      end
      throw :halt, [code, headers, [body.to_s]]
    end

    # `pass` — stop handling this route and let the next matching route
    # take the request. Real Sinatra throws :pass; dispatch catches it
    # per-route and continues the search.
    def pass
      throw :pass
    end

    # ---- dispatch ----

    def dispatch(env)
      @env     = env
      @status  = 200
      @headers = { "Content-Type" => "text/html" }
      verb     = env["REQUEST_METHOD"]
      segs     = (env["PATH_INFO"] || "/").split("/").reject { |s| s.empty? }
      # Real Sinatra merges query-string AND form-body params (and path
      # captures) into one `params` hash.
      base_params = parse_query(env["QUERY_STRING"]).merge(parse_form_body)

      # `halt`/`redirect` `throw :halt, triplet`; the catch returns it.
      catch(:halt) do
        begin
          matched = nil
          self.class.routes.each do |route_verb, pattern, block|
            next unless route_verb == verb
            captured = match(pattern, segs)
            next unless captured
            @params = base_params.merge(captured)
            run_filters
            # `pass` inside the block throws :pass; catch it here and
            # fall through to the next matching route. A normal return is
            # wrapped in [:done, triplet] so it's distinguishable from the
            # nil that catch(:pass) yields on a throw.
            outcome = catch(:pass) { [:done, finalize(instance_exec(&block))] }
            if outcome.is_a?(Array) && outcome[0] == :done
              matched = outcome[1]
              break
            end
            # passed -> try the next route
          end

          if matched
            matched
          else
            # No route matched -> custom not_found or default 404.
            nf = self.class.not_found_handler
            if nf
              @params = base_params
              @status = 404
              finalize(instance_exec(&nf))
            else
              [404, { "Content-Type" => "text/plain" }, ["Not Found\n"]]
            end
          end
        rescue UncaughtThrowError
          # A `halt`/`redirect` throw — let the enclosing catch(:halt)
          # take it, not the error-handler path below.
          raise
        rescue => e
          # Route raised — dispatch to a registered `error` handler if one
          # matches the exception's class; otherwise re-raise (the server
          # maps it to a 500).
          handler = error_handler_for(e)
          raise unless handler
          @sinatra_error = e
          @status = 500 # default error status; the handler may override
          finalize(instance_exec(&handler))
        end
      end
    end

    # Wrap a route's return value into a Rack body triplet. A streaming
    # body (`stream { ... }`, responds to `call`) is passed through as-is;
    # any other value becomes a one-element string body.
    def finalize(body)
      rack_body = body.respond_to?(:call) ? body : [body.to_s]
      [@status, @headers, rack_body]
    end

    def run_filters
      self.class.filters.each { |f| instance_exec(&f) }
    end

    def error_handler_for(exc)
      self.class.error_handlers.each do |klass, block|
        return block if exc.is_a?(klass)
      end
      nil
    end

    # The exception currently being handled (Sinatra exposes it via
    # env["sinatra.error"]; we also keep it here for helpers).
    def request
      @request ||= Request.new(@env)
    end

    # `settings` returns the application class itself, mirroring
    # real Sinatra. Combined with `set :key, val` defining a
    # singleton method on the class, this lets plugin / route
    # code do `settings.key` (read) and
    # `settings.respond_to?(:key)` (presence check). Stored values
    # live on the class's `settings_store` Hash; the singleton-
    # method-per-key indirection produces the symmetric
    # respond_to? predicate sinatra-jsonp relies on.
    def settings
      self.class
    end

    # Sinatra's streaming helper: `stream { |out| out << chunk }`.
    def stream(&block)
      StreamingBody.new(block)
    end

    # Parse an `application/x-www-form-urlencoded` request body into params
    # (POST/PUT/PATCH only), mirroring real Sinatra/Rack.
    def parse_form_body
      verb = @env["REQUEST_METHOD"]
      return {} unless ["POST", "PUT", "PATCH"].include?(verb)
      ct = @env["CONTENT_TYPE"] || ""
      return {} unless ct.include?("application/x-www-form-urlencoded")
      parse_query(request_body)
    end

    # Returns a params Hash on match (incl. "splat" => [...]), or nil.
    def match(pattern, segs)
      return nil unless pattern.length == segs.length
      params = {}
      splat = []
      i = 0
      while i < pattern.length
        kind = pattern[i][0]
        seg  = segs[i]
        if kind == :lit
          return nil unless pattern[i][1] == seg
        elsif kind == :cap
          params[pattern[i][1]] = unescape(seg)
        else # :splat
          splat << unescape(seg)
        end
        i += 1
      end
      params["splat"] = splat unless splat.empty?
      params
    end

    def params
      @params ||= {}
    end

    def request_body
      io = @env && @env["rack.input"]
      io ? io.read.to_s : ""
    end

    # "a=1&b=hello+world&flag" -> {"a"=>"1","b"=>"hello world","flag"=>""}
    def parse_query(qs)
      out = {}
      return out if qs.nil? || qs.empty?
      qs.split("&").each do |pair|
        next if pair.empty?
        # `split("=", 2)` keeps any "=" in the value intact (GAP #9, now
        # fixed in the engine — previously this needed a manual index slice).
        key, val = pair.split("=", 2)
        out[unescape(plus_to_space(key))] = unescape(plus_to_space(val || ""))
      end
      out
    end

    def plus_to_space(str)
      str.gsub("+", " ")
    end

    # Minimal percent-decoding (e.g. %3C -> "<").
    def unescape(str)
      out = ""
      i = 0
      while i < str.length
        ch = str[i]
        if ch == "%" && i + 2 < str.length + 1
          hex = str[(i + 1)..(i + 2)]
          if hex && hex.length == 2
            out << hex.to_i(16).chr
            i += 3
            next
          end
        end
        out << ch
        i += 1
      end
      out
    end
  end

  # `Sinatra.helpers Module` — module-level convenience that
  # forwards to `Sinatra::Base.helpers`. Real Sinatra exposes
  # both (`Sinatra::Base.helpers Mod` is the canonical class-
  # method form; `Sinatra.helpers Mod` is sugar that registers
  # the helpers on the default Sinatra::Application). For our
  # micro-Sinatra the simplest equivalent is to forward to
  # Sinatra::Base — subclassing apps inherit via the class
  # chain. Used by `sinatra-jsonp` and a few other gems whose
  # plugin file ends with `Sinatra.helpers PluginModule` at
  # module level.
  def self.helpers(*modules)
    Sinatra::Base.helpers(*modules)
  end

  # `Sinatra.register Module` — same forwarding shape as
  # `Sinatra.helpers` above. Not used by sinatra-jsonp directly
  # but ships for parity with the gem ecosystem; plugin files
  # that end with `register PluginModule` at top of `module
  # Sinatra` are the canonical authoring shape.
  def self.register(*extensions)
    Sinatra::Base.register(*extensions)
  end
end
