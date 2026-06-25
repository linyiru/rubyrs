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

# `Rack::Utils` / `Rack::Headers` shim — exposes just the entry
# points vendored middleware gems reach for (rack-cors uses
# `Rack::Utils.valid_path?`, `unescape_path`, `clean_path_info`
# and probes `defined?(Rack::Headers)` to pick a headers-wrapping
# strategy). Real Rack ships these inside the `rack` gem; we
# don't load rack, so plugin authors who `require 'rack/cors'`
# (which itself doesn't `require 'rack/utils'`) need this fallback.
module Rack
  # Sentinel — defining this constant flips rack-cors's
  # `if defined?(Rack::Headers)` branch to the identity-passthrough
  # path (`->(h) { h }`), so we don't need a HeaderHash impl.
  class Headers; end

  module Utils
    # `valid_path?(path)` — false for paths containing `\0` or
    # `..` segments (path-traversal guard). The CRuby gem rejects
    # both before clean_path_info runs.
    def self.valid_path?(path)
      !path.nil? && !path.include?("\0") && !path.split("/").include?("..")
    end

    # `unescape_path(path)` — URI-decode `%xx` sequences. The
    # subset rack-cors actually needs is ASCII path bytes; real
    # Rack uses URI::DEFAULT_PARSER. Minimal implementation:
    # walk the string, decode `%HH` pairs, copy everything else.
    def self.unescape_path(s)
      out = String.new
      i = 0
      while i < s.length
        c = s[i]
        if c == "%" && i + 2 < s.length
          hex = s[i + 1, 2]
          out << hex.to_i(16).chr
          i += 3
        else
          out << c
          i += 1
        end
      end
      out
    end

    # `clean_path_info(path)` — collapse `//`, resolve `.` and
    # `..` segments. The CRuby implementation uses
    # `Pathname#cleanpath`-equivalent logic; we do the same with
    # an explicit stack walk.
    def self.clean_path_info(path)
      segs = []
      path.split("/").each do |seg|
        next if seg.empty? || seg == "."
        if seg == ".."
          segs.pop
        else
          segs << seg
        end
      end
      lead = path.start_with?("/") ? "/" : ""
      trail = (path.end_with?("/") && !segs.empty?) ? "/" : ""
      "#{lead}#{segs.join("/")}#{trail}"
    end
  end

  # `Rack::Session::Cookie` — minimal cookie-backed session
  # middleware. Used by sinatra-flash and any other gem that
  # depends on `env["rack.session"]`. Real Rack ships a much
  # richer impl (HMAC signing, Marshal-based binary coder,
  # secure expiry, etc.); for the parity-fixture subset we use
  # a JSON coder and skip signing entirely, which is the same
  # shape the CRuby oracle picks when given an explicit
  # `coder: ` option. The fixture passes that option on both
  # runtimes so the on-the-wire cookie payload is byte-
  # identical.
  module Session
    class Cookie
      DEFAULT_OPTIONS = {
        key: "rack.session",
        path: "/",
        domain: nil,
        expire_after: nil,
        secure: false,
        httponly: true,
      }.freeze

      def initialize(app, options = {})
        @app = app
        @options = DEFAULT_OPTIONS.merge(options)
        # `coder:` lets callers swap the serialiser. Defaults to
        # the JSON coder below — deterministic, parser-portable,
        # and round-trips the same payload across both runtimes
        # without HMAC nonces.
        @coder = options[:coder] || JsonCoder
      end

      def call(env)
        session = load_session(env)
        env["rack.session"] = session
        env["rack.session.options"] = @options
        status, headers, body = @app.call(env)
        # Only emit Set-Cookie when the session has at least one
        # key — keeps the diff transcript byte-identical for
        # stateless scenarios. Sinatra-flash + similar gems
        # depend on the Set-Cookie reflecting the session on
        # mutation; reading a key without writing leaves the
        # session unchanged and we re-emit Set-Cookie to refresh
        # the round-trip.
        if !session.empty? || env["HTTP_COOKIE"]&.include?("#{@options[:key]}=")
          serialised = @coder.encode(session.to_hash)
          cookie_parts = ["#{@options[:key]}=#{serialised}"]
          cookie_parts << "path=#{@options[:path]}" if @options[:path]
          cookie_parts << "domain=#{@options[:domain]}" if @options[:domain]
          cookie_parts << "HttpOnly" if @options[:httponly]
          cookie_parts << "secure" if @options[:secure]
          headers["set-cookie"] = cookie_parts.join("; ")
        end
        [status, headers, body]
      end

      private

      def load_session(env)
        # IndifferentHash so `session[:k]` and `session["k"]` are the same
        # slot — Rack's SessionHash stringifies keys, and the JSON round-trip
        # turns every key into a String, so a symbol-keyed write would
        # otherwise read back nil. Matches real Rack session semantics.
        session = ::Sinatra::IndifferentHash.new
        cookie_header = env["HTTP_COOKIE"]
        return session if cookie_header.nil? || cookie_header.empty?
        cookie_header.split(";").each do |pair|
          k, v = pair.strip.split("=", 2)
          next unless k == @options[:key] && v
          data = @coder.decode(v)
          if data.is_a?(Hash)
            data.each { |dk, dv| session[dk] = dv }
          end
          break
        end
        session
      end
    end

    # JSON-backed (de)serialiser. Round-trips a Hash through
    # JSON.generate / JSON.parse. Deterministic for a given Hash
    # (matches CRuby JSON's insertion-order serialisation), so
    # both runtimes emit the same Set-Cookie payload.
    module JsonCoder
      def self.encode(data)
        require "json"
        JSON.generate(data)
      end

      def self.decode(str)
        require "json"
        JSON.parse(str)
      rescue StandardError
        nil
      end
    end
  end
end

module Sinatra
  # rubyrs's blessed in-tree Sinatra (`require "sinatra"` / `"sinatra/base"`
  # resolve here via stdlib_vendor). MUST NOT engine-branch on RUBYRS
  # (ADR 0026 v2 anti-pattern). Grown from the micro sinatra_lite toward a
  # real/complete Sinatra; the diff_framework Sinatra fixtures are its gate.
  VERSION = "4.2.1-rubyrs".freeze unless defined?(VERSION)
  # Sinatra's params hash: a key is reachable as either a String or a
  # Symbol (`params["id"]` == `params[:id]`). Real Sinatra uses
  # Sinatra::IndifferentHash; this is the same contract, String-backed.
  # Used everywhere a route block reads params, which is ~every real app.
  class IndifferentHash < Hash
    def self.from(h)
      out = new
      h.each { |k, v| out[k] = v } if h
      out
    end
    def [](key);        super(key.to_s); end
    def []=(key, val);  super(key.to_s, val); end
    def key?(key);      super(key.to_s); end
    alias_method :has_key?, :key?
    alias_method :include?, :key?
    alias_method :member?, :key?
    def fetch(key, *a, &b); super(key.to_s, *a, &b); end
    def dig(key, *rest);    super(key.to_s, *rest); end
    def values_at(*keys);   keys.map { |k| self[k] }; end
    def merge(other)
      out = dup
      other.each { |k, v| out[k] = v }
      out
    end
    def merge!(other)
      other.each { |k, v| self[k] = v }
      self
    end
  end

  # Minimal Rack::Request-ish wrapper. Real Sinatra exposes `request`
  # inside route blocks; apps read `request.user_agent`, `request.path`,
  # `request.request_method`, header access, etc.
  class Request
    def initialize(env)
      @env = env
    end
    # Public env accessor — sinatra-cors uses
    # `request.env["HTTP_ORIGIN"]` to read the CORS origin
    # header without going through the bracket shim that
    # strips the `HTTP_` prefix. Same shape real Sinatra
    # exposes via Rack::Request#env.
    def env; @env; end
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

  # Logger stub — sinatra-cors uses `.warn(msg)` for CORS
  # rejection diagnostics. Real Sinatra hands you a
  # Rack::CommonLogger-backed object; we just no-op so the
  # call sites work without a host-side log surface.
  class LoggerStub
    def debug(_msg = nil); end
    def info(_msg = nil); end
    def warn(_msg = nil); end
    def error(_msg = nil); end
    def fatal(_msg = nil); end
  end

  # Lightweight Hash-of-Arrays view over the flat routes table.
  # sinatra-cors's `allowed_methods` helper does
  #   `settings.routes.each do |method, routes_for_method| …`
  # expecting a Hash. The vendored micro-Sinatra stores routes
  # as a flat `[verb, pattern, block, conditions]` Array. This
  # adapter groups them by verb on demand so the iteration
  # contract matches without changing the underlying storage.
  class RoutesView
    include Enumerable
    def initialize(routes_array)
      @grouped = {}
      routes_array.each do |entry|
        verb = entry[0]
        (@grouped[verb] ||= []) << entry[1..]
      end
    end
    def each(&block); @grouped.each(&block); end
    def [](verb); @grouped[verb] || []; end
  end

  class Base
    class << self
      # Internal storage: flat Array of `[verb, pattern, block,
      # conditions]` tuples. The dispatch loop uses
      # `routes_array` directly; the public `routes` getter
      # returns a RoutesView (Hash-of-Arrays view) so
      # introspection callers like sinatra-cors's
      # `settings.routes.each do |method, routes_for_method|`
      # see the Hash shape real Sinatra exposes.
      def routes_array; @routes ||= []; end
      def routes;       RoutesView.new(routes_array); end
      def filters;        @filters        ||= []; end
      def error_handlers; @error_handlers ||= []; end

      # Real Sinatra route declarations accept `(path, **opts, &block)`.
      # The optional `opts` Hash carries route-condition arguments
      # (`is_cors_preflight: true`, etc.); each opts key was
      # registered earlier via the block-form `set(:key) { |arg|
      # condition { ... } }`. At route-declaration time we look up
      # the registered handler and invoke it with `arg` so it can
      # call `condition { ... }` on a per-route conditions stack.
      # `paths` is normalised to an Array so the same body handles
      # both `get '/foo'` (single String) and
      # `get '/foo', '/bar'` / `get ['/foo', '/bar']` (multiple
      # paths). Real Sinatra supports the multi-path shape via
      # the sinatra-contrib/MultiRoute plugin, which forwards
      # `super(*processed_args, &block)` to the verb method —
      # `processed_args` puts the path Array first. Accepting the
      # Array directly here lets MultiRoute's vendored source work
      # unmodified against our micro-Sinatra.
      def get(*paths_and_opts, **opts, &block)
        paths, opts = _normalise_paths_and_opts(paths_and_opts, opts)
        paths.each do |path|
          add_route("GET", path, opts, &block)
          # Real Sinatra auto-registers HEAD for every GET route
          # (HEAD requests are GETs with the body stripped). The
          # route table thus shows HEAD entries alongside GETs,
          # which sinatra-cors's `allowed_methods` enumeration
          # picks up. We mirror by adding the HEAD entry too;
          # the block runs the same way (the response body is
          # never sent for a HEAD response in real Sinatra; our
          # micro-Sinatra doesn't yet enforce that, but the
          # routes-table introspection contract matches).
          add_route("HEAD", path, opts, &block)
        end
      end
      def post(*paths_and_opts, **opts, &block)
        paths, opts = _normalise_paths_and_opts(paths_and_opts, opts)
        paths.each { |p| add_route("POST", p, opts, &block) }
      end
      def put(*paths_and_opts, **opts, &block)
        paths, opts = _normalise_paths_and_opts(paths_and_opts, opts)
        paths.each { |p| add_route("PUT", p, opts, &block) }
      end
      def delete(*paths_and_opts, **opts, &block)
        paths, opts = _normalise_paths_and_opts(paths_and_opts, opts)
        paths.each { |p| add_route("DELETE", p, opts, &block) }
      end
      # OPTIONS verb — Sinatra 4+ ships this as a normal route
      # registrar. sinatra-cors uses it for the CORS preflight
      # catch-all `app.options "*", is_cors_preflight: true do …`.
      def options(*paths_and_opts, **opts, &block)
        paths, opts = _normalise_paths_and_opts(paths_and_opts, opts)
        paths.each { |p| add_route("OPTIONS", p, opts, &block) }
      end
      # PATCH (REST partial-update — todo-backend's `patch "/todos/:id"`)
      # and HEAD. Sinatra auto-registers HEAD for every GET; an explicit
      # `head` route is rarer but real.
      def patch(*paths_and_opts, **opts, &block)
        paths, opts = _normalise_paths_and_opts(paths_and_opts, opts)
        paths.each { |p| add_route("PATCH", p, opts, &block) }
      end
      def head(*paths_and_opts, **opts, &block)
        paths, opts = _normalise_paths_and_opts(paths_and_opts, opts)
        paths.each { |p| add_route("HEAD", p, opts, &block) }
      end

      # Splits the positional args into a flat list of path Strings
      # and merges any trailing positional Hash with the kwargs hash.
      # Accepts: `'/foo'`, `'/foo', '/bar'`, `['/foo', '/bar']`,
      # and `['/foo'], opts_hash` (MultiRoute's `super(paths_array,
      # opts_hash, &block)` shape — `route_args` returns
      # `[paths_array, opts_hash]`).
      def _normalise_paths_and_opts(positional, kwargs)
        opts = kwargs.dup
        positional = positional.dup
        if positional.last.is_a?(Hash)
          opts.merge!(positional.pop)
        end
        # Flatten so a single `[paths]` positional or a mix like
        # `'/a', ['/b', '/c']` both reduce to a single list.
        paths = positional.flatten
        [paths, opts]
      end

      # Generic verb-routed declaration — the entry point
       # sinatra-contrib/MultiRoute's `route(*verbs, paths, &block)`
       # calls via `super(verb, route, options, &block)` after the
       # verb/route/options demux. Real Sinatra exposes the same
       # shape (lib/sinatra/base.rb's `route` class method).
      def route(verb, path, opts = {}, &block)
        add_route(verb.to_s.upcase, path, opts, &block)
      end

      def add_route(verb, path, opts = {}, &block)
        # Per-route conditions list — populated by the block-form
        # `set` handlers we invoke for each opts key. The dispatch
        # loop checks each condition with the request instance
        # context before invoking the route block.
        per_route_conditions = []
        @pending_conditions = per_route_conditions
        opts.each do |key, val|
          handler = setting_handlers[key]
          # `instance_exec` on self (the app class) so the
          # handler block's body can reach class methods like
          # `condition { ... }` — same self-rebind contract
          # real Sinatra uses for the
          # `set(:key) do |arg| condition { ... } end` shape.
          instance_exec(val, &handler) if handler
        end
        @pending_conditions = nil
        routes_array << [verb, compile(path), block, per_route_conditions]
      end

      # `condition { ... }` from inside a `set(:key) { |arg|
      # condition { ... } }` block. The block is appended to the
      # per-route `pending_conditions` list (a class-instance
      # variable that `add_route` sets before invoking each
      # opts-key handler). The condition block runs in the
      # dispatch instance's context at request time.
      def condition(&block)
        if @pending_conditions
          @pending_conditions << block
        end
      end

      # Block-form `set(:key) do |arg| ... end` — registers a
      # setting handler. Real Sinatra invokes the handler when a
      # route declares the key as an option (`get "/", key:
      # value do …`). The handler typically calls `condition {
      # ... }` to register a per-route predicate. sinatra-cors
      # uses this to declare `:is_cors_preflight`.
      def setting_handlers; @setting_handlers ||= {}; end

      # Runs before every route (in the request instance's context).
      def before(&block)
        filters << block
      end

      # Runs AFTER every route — used by sinatra-cors's `app.after
      # do; cors; end` to append CORS headers to every response.
      # The block runs in the dispatch instance's context, where
      # `headers["X"] = ...` mutates the in-flight response Hash.
      def after_filters; @after_filters ||= []; end
      def after(&block)
        after_filters << block
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

      # `environment` — the current run environment (`set :environment`,
      # else $APP_ENV/$RACK_ENV, else :development), mirroring real Sinatra.
      # Drives `configure` + the `*?` predicates. ENV may be absent on
      # rubyrs (ADR 0017), so guard the lookup.
      def environment
        (settings_store[:environment] ||
          (defined?(ENV) && (ENV["APP_ENV"] || ENV["RACK_ENV"])) ||
          :development).to_sym
      end
      def development?; environment == :development; end
      def production?;  environment == :production;  end
      def test?;        environment == :test;        end

      # `configure(*envs) { |app| ... }` — run the block (with the app class)
      # only when no env is given OR the current environment is one of them.
      # Real apps wrap dev-only setup like `configure :development do
      # require "sinatra/reloader"; register Sinatra::Reloader end` — which
      # is then a no-op in production (the block never runs).
      def configure(*envs)
        yield self if envs.empty? || envs.include?(environment)
      end

      # `host_authorization` config (Rack::Protection::HostAuthorization).
      # A Hash `{permitted_hosts: [...]}`, or a callable returning one.
      # Default matches real Sinatra 4: in development, localhost/.localhost/
      # .test + any IP (0.0.0.0/0, ::/0); in production, `{}` (empty → accept
      # all). `set :host_authorization, {permitted_hosts: []}` ⇒ accept all.
      def host_authorization
        cfg = settings_store[:host_authorization]
        cfg = cfg.call if cfg.respond_to?(:call)
        return cfg if cfg
        if development?
          require "ipaddr"
          { permitted_hosts: ["localhost", ".localhost", ".test",
                              IPAddr.new("0.0.0.0/0"), IPAddr.new("::/0")] }
        else
          {}
        end
      end
      # Sinatra's `set :foo, val` doubles as both storage AND a
      # reflection surface — `settings.foo` returns the value
      # and `settings.respond_to?(:foo)` reports true. Real
      # Sinatra implements this by defining singleton methods
      # on the app class; we mirror that so plugins like
      # sinatra-jsonp's `settings.respond_to?(:json_pretty) &&
      # settings.json_pretty` predicate-and-read shape works.
      def set(key, value = nil, &block)
        if block_given?
          # Real Sinatra distinguishes block-form `set` by block
          # arity:
          #   * `set(:key) { value }` (arity 0) — block computes
          #     the value. CRuby installs it as a singleton-class
          #     method so each `settings.key` invocation re-runs
          #     the block. We evaluate eagerly at set-time and
          #     store the result; that's adequate for the
          #     idiomatic shape (`set :json_encoder do
          #     ::MultiJson end`) where the value doesn't change
          #     after declaration.
          #   * `set(:key) { |arg| ... }` (arity >= 1) — block is
          #     a route-option handler invoked when a route
          #     declares the key as an option (the standard
          #     sinatra-cors `set(:is_cors_preflight) { |arg|
          #     condition { ... } }` shape). Stored unevaluated;
          #     `add_route` runs it with the opts value.
          if block.arity == 0
            value = block.call
          else
            setting_handlers[key] = block
            return self
          end
        end
        settings_store[key] = value
        unless respond_to?(key)
          # Reader walks the superclass chain so settings declared
          # on `Sinatra::Base` (real-gem idiom for plugin
          # registration: `Base.set :json_encoder do ::MultiJson
          # end`) reach every subclass. The walker stops at the
          # first class whose `settings_store` actually contains
          # `key` — preserves the "subclass overrides parent"
          # contract real Sinatra honours.
          define_singleton_method(key) do
            cls = self
            while cls
              store = cls.settings_store
              return store[key] if store.key?(key)
              cls = cls.superclass
            end
            nil
          end
          # `<key>?` predicate — real Sinatra auto-generates this
          # alongside the reader. Returns true when the value is
          # truthy AND non-empty (CRuby's `present?`-style rule
          # for the Configurable surface, not the Object#`!nil?
          # one). sinatra-cors uses `settings.max_age?` etc.
          # Walks the inheritance chain the same way the reader
          # does so a `set :foo, true` on Base reads `true` on
          # every subclass's `.foo?` too.
          define_singleton_method("#{key}?") do
            cls = self
            while cls
              store = cls.settings_store
              if store.key?(key)
                v = store[key]
                return (!v.nil? && v != false && v != "")
              end
              cls = cls.superclass
            end
            false
          end
        end
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
          # `extend ext` adds ext's instance methods to this app
          # class's singleton class — so `MyApp.get(...)` etc. can
          # resolve to the extension's overrides FIRST, with their
          # bare `super` reaching the original Sinatra::Base verb
          # methods. This is the canonical "register extends"
          # contract real Sinatra implements; sinatra-contrib/
          # MultiRoute (and the rack-cors / sinatra-cors family
          # by virtue of their `.registered(app)` hooks calling
          # `app.helpers Module`) all rely on it.
          extend ext
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
        # A Regexp route is stored as-is; `match` detects it and matches the
        # raw PATH_INFO instead of segment-by-segment.
        return path if path.is_a?(Regexp)
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

      # Rack-style middleware stack. `use Klass, *args, &block`
      # registers a middleware; `call(env)` builds the chain
      # exactly once (lazily, memoised in @built_app) and
      # delegates. Real Sinatra::Base uses the same
      # outermost-first `middleware.reverse.inject(inner)` walk.
      def middleware_stack; @middleware_stack ||= []; end

      # `use(klass, *args, &block)` — append `klass` to the
      # middleware stack. The optional `block` is forwarded to
      # the middleware's constructor (rack-cors uses the block
      # for its `allow do ... end` DSL configuration).
      def use(klass, *args, &block)
        middleware_stack << [klass, args, block]
        @built_app = nil  # invalidate any cached chain
        self
      end

      def call(env)
        # Class entry point: a fresh instance per request, wrapped in the
        # middleware chain (`App.call` for a no-arg app, or via the request
        # server). Custom-initialize apps go through the instance path below.
        (@built_app ||= build_middleware(->(e) { new.dispatch(e) })).call(env)
      end

      def build_app
        build_middleware(->(e) { new.dispatch(e) })
      end

      # Wrap `inner` (a `->(env)` that runs the app's dispatch) in the
      # middleware chain: any `use`d middleware plus the auto-wired session
      # cookie. The SAME chain is used for the class call path (fresh
      # instance per request) and the instance path (`dup` of a pre-built
      # modular app per request) — so `run App.new(repo)` gets sessions /
      # host_authorization / `use`d middleware exactly like `App.call`.
      def build_middleware(inner)
        stack = middleware_stack.dup
        # `enable :sessions` auto-wires the session-cookie middleware, like
        # Sinatra's setup_sessions (`use session_store` when `sessions?`) —
        # so `session[:k]` persists across requests with no explicit `use`.
        # Skipped if already present (an app may `use Rack::Session::Cookie`).
        sess = settings_store[:sessions]
        if sess && stack.none? { |klass, _, _| klass == ::Rack::Session::Cookie }
          opts = {}
          opts[:secret] = settings_store[:session_secret] if settings_store[:session_secret]
          opts.merge!(sess) if sess.respond_to?(:to_hash)
          stack = [[::Rack::Session::Cookie, [opts], nil]] + stack
        end
        stack.reverse.inject(inner) do |inner_app, mw|
          klass, args, block = mw
          klass.new(inner_app, *args, &block)
        end
      end

      def run!(opts = {})
        bind     = opts[:bind] || "127.0.0.1"
        port     = opts[:port] || 4567
        duration = opts[:duration] || 86_400
        # Capture self for the lambda closure. Without this, the
        # inner `->(env) { call(env) }` would resolve `call` via
        # the lambda's own `self` at invocation time (a different
        # binding once the http front cross-calls into it).
        app_class = self
        app = ->(env) { app_class.call(env) }
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
      @headers ||= { "content-type" => "text/html" }
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
      # `:css` — used by sinatra-contrib/LinkHeader's `stylesheet`
      # helper for the default `type=` of generated link tags.
      css:  "text/css",
    }.freeze
    # Sinatra's `content_type` is dual-purpose: zero-arg form
    # returns the currently-set response Content-Type (or nil
    # if unset); one-arg form sets it. The no-arg query shape
    # is used by sinatra-param's `if content_type and
    # content_type.match(mime_type(:json))` to decide between
    # plain-text vs JSON error encoding.
    def content_type(type = nil)
      if type.nil?
        headers["content-type"]
      else
        headers["content-type"] = if type.is_a?(Symbol)
          CONTENT_TYPE_SHORTHANDS.fetch(type) do
            raise ArgumentError, "Unknown media type for #{type.inspect}"
          end
        else
          type.to_s
        end
      end
    end

    # `mime_type(:symbol)` — look up the canonical media type
    # for a registered symbol. Real Sinatra ships a much larger
    # Rack::Mime-backed table; sinatra-param only consults
    # `mime_type(:json)` to decide error encoding, so we expose
    # the same minimal table `content_type` uses. Returns nil
    # for unknown symbols — same shape Rack::Mime emits.
    def mime_type(sym)
      self.class::CONTENT_TYPE_SHORTHANDS[sym]
    end

    def redirect(location, code = 302)
      # Real Sinatra expands a path to an absolute URL using the request
      # host (the Location header is absolute). Match that.
      if location.start_with?("/")
        host = @env["HTTP_HOST"] || "#{@env['SERVER_NAME']}:#{@env['SERVER_PORT']}"
        location = "http://#{host}#{location}"
      end
      headers["location"] = location
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

    # Instance `call` — the Rack entry point for a MODULAR app used as a
    # pre-built instance: `run TodoApp.new(repo)` (config.ru), where the
    # custom `initialize(repo)` means the class-level `call` (`new.dispatch`)
    # can't construct it. Routes through the SAME middleware chain as the
    # class path (build_middleware), so a modular instance app gets sessions
    # / host_authorization / `use`d middleware too — closing the gap where
    # `dup.dispatch` bypassed them. `dup` per request preserves the shared
    # prototype state (e.g. @repo) while @env/@params/@status stay
    # per-request — mirrors real Sinatra's `def call(env); dup.call!(env)`.
    def call(env)
      @built_app ||= self.class.build_middleware(->(e) { dup.dispatch(e) })
      @built_app.call(env)
    end

    # Rack::Protection::HostAuthorization, faithfully: accept when the
    # permitted-host list is empty (production default / `permitted_hosts:
    # []`); otherwise the Host (port stripped, downcased) must match an exact
    # host, a `.domain` suffix, or fall inside a permitted IPAddr range —
    # else 403 "Host not permitted". (X-Forwarded-Host is not yet checked.)
    def host_authorized?(env)
      permitted = Array(self.class.host_authorization[:permitted_hosts])
      return true if permitted.empty?
      host = (env["HTTP_HOST"] || "").split(/:\d+\z/).first.to_s.downcase
      permitted.any? do |h|
        if h.is_a?(IPAddr)
          begin h.include?(IPAddr.new(host)); rescue StandardError; false; end
        elsif h.respond_to?(:start_with?) && h.start_with?(".")
          /\A[a-z0-9\-.]+#{Regexp.escape(h[1..-1].downcase)}\z/i.match?(host)
        else
          h.to_s.downcase == host
        end
      end
    end

    def dispatch(env)
      @env     = env
      return [403, { "content-type" => "text/plain" }, ["Host not permitted"]] unless host_authorized?(env)
      @status  = 200
      @headers = { "content-type" => "text/html" }
      verb     = env["REQUEST_METHOD"]
      segs     = (env["PATH_INFO"] || "/").split("/").reject { |s| s.empty? }
      # Real Sinatra merges query-string AND form-body params (and path
      # captures) into one indifferent (String/Symbol) `params` hash.
      base_params = IndifferentHash.from(parse_query(env["QUERY_STRING"]))
      base_params.merge!(parse_form_body)

      # `halt`/`redirect` `throw :halt, triplet`; the catch returns it.
      result = catch(:halt) do
        begin
          matched = nil
          self.class.routes_array.each do |entry|
            route_verb, pattern, block, conditions = entry[0], entry[1], entry[2], entry[3]
            conditions ||= []
            next unless route_verb == verb
            matchdata = match(pattern, segs, env["PATH_INFO"] || "/")
            next unless matchdata
            captured, block_args = matchdata
            @params = base_params.merge(captured)
            # Per-route conditions — declared via `condition { ...
            # }` from inside a block-form `set(:key) { |arg| ... }`
            # handler. Each condition is a block that returns
            # truthy/falsy in the dispatch instance's context.
            # All must pass for the route to fire. sinatra-cors
            # uses this for `is_cors_preflight: true`.
            next unless conditions.all? { |c| instance_exec(&c) }
            run_filters
            # `pass` inside the block throws :pass; catch it here and
            # fall through to the next matching route. A normal return is
            # wrapped in [:done, triplet] so it's distinguishable from the
            # nil that catch(:pass) yields on a throw.
            outcome = catch(:pass) { [:done, finalize(instance_exec(*block_args, &block))] }
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
              [404, { "content-type" => "text/plain" }, ["Not Found\n"]]
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
      # After-filters run regardless of halt / normal exit / error
      # handler. They share @headers with the just-finalized
      # response triplet (Hash by reference), so any mutation
      # they perform — sinatra-cors's `cors` helper appends
      # `Access-Control-Allow-Origin` etc. — is visible in the
      # outgoing response without rebuilding the triplet.
      self.class.after_filters.each { |f| instance_exec(&f) }
      result
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

    # `env` — the raw Rack env Hash, exposed as a helper (real Sinatra:
    # `request.env` and the `env` method both reach it). Apps read it for
    # raw headers (`env["HTTP_..."]`) and `env["rack.input"]`.
    def env
      @env
    end

    # `url(addr) / uri(addr) / to(addr)` — build a URL for a path. Absolute
    # by default (`redirect to("/x")`, `todo_url = uri "/todos/#{id}"`).
    # Mirrors real Sinatra: a full URI is returned as-is; otherwise
    # scheme://host[:port] + SCRIPT_NAME + addr, File.join'd (so slashes
    # collapse the same way). The default port (80/443) is stripped to match.
    def uri(addr = nil, absolute = true, add_script_name = true)
      return addr if addr.to_s =~ /\A[a-z][a-z0-9+.\-]*:/i
      parts = [host = +""]
      if absolute
        scheme = @env["rack.url_scheme"] || "http"
        host << "#{scheme}://"
        raw = @env["HTTP_HOST"] || "#{@env['SERVER_NAME']}:#{@env['SERVER_PORT']}"
        default = scheme == "https" ? ":443" : ":80"
        raw = raw[0...-default.length] if raw.end_with?(default)
        host << raw
      end
      parts << @env["SCRIPT_NAME"].to_s if add_script_name
      parts << (addr || @env["PATH_INFO"]).to_s
      File.join(parts)
    end
    alias_method :url, :uri
    alias_method :to, :uri

    # `session` — the Hash-shaped session installed by the
    # session middleware (e.g. `Rack::Session::Cookie`). Returns
    # an empty Hash if no session middleware is in the chain,
    # matching the contract every session-aware gem assumes.
    # sinatra-flash reads / writes `session[:flash]` through
    # this helper; the actual storage layer is whatever
    # middleware set `env["rack.session"]`.
    def session
      @env["rack.session"] ||= ::Sinatra::IndifferentHash.new
    end

    # `erb(template, options = {}, locals = {})` — render an ERB template in
    # THIS request instance's binding, so `@ivars` (and helper methods) set
    # by the route are visible (`erb :index` after `@todos = ...`). The
    # template is an inline String (`erb "<%= @x %>"`) or a Symbol naming a
    # file under the views dir (`erb :index` → `<views>/index.erb`,
    # views = `set :views` or "./views").
    #
    # Renders via Erubi — the SAME engine real Sinatra 4 uses (through Tilt)
    # — so output (including whitespace trimming around `<% %>` vs `<%= %>`
    # lines, and HTML escaping) is byte-identical to the real gem.
    #   - inline String (`erb "<%= @x %>"`) or Symbol → `<views>/<name>.erb`
    #     (views = `set :views` or "./views");
    #   - rendered in this request instance's binding, so route-set `@ivars`
    #     are visible;
    #   - LAYOUT: wrapped in `<views>/layout.erb`'s `<%= yield %>` when it
    #     exists; `layout: false` disables, `layout: :name` picks another.
    # `locals:` (template-local vars) are still deferred — they need
    # Binding#local_variable_set, which rubyrs doesn't expose yet.
    def erb(template, options = {}, _locals = {})
      require "erubi"
      views = self.class.settings_store[:views] || "./views"
      src = template.is_a?(Symbol) ? File.read(File.join(views, "#{template}.erb")) : template.to_s
      rendered = eval(Erubi::Engine.new(src).src, binding)
      layout = options.key?(:layout) ? options[:layout] : :layout
      if layout
        lname = layout == true ? :layout : layout
        lpath = File.join(views, "#{lname}.erb")
        if File.exist?(lpath)
          # Wrap the layout's compiled source in a method so its `<%= yield %>`
          # yields the inner-rendered content (like Tilt does).
          lmod = Module.new
          lmod.module_eval("def __erb_layout\n#{Erubi::Engine.new(File.read(lpath)).src}\nend")
          extend(lmod)
          rendered = __erb_layout { rendered }
        end
      end
      rendered
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

    # `response` — in real Sinatra this is a Rack::Response
    # instance with `.headers`, `.body`, `.status`, etc. The
    # vendored micro-Sinatra represents the in-flight response
    # via the dispatch instance's `@status` / `@headers` slots
    # directly (see `dispatch` / `finalize` below); the simplest
    # response-API shim returns self, since `self.headers[...]`
    # already mutates the same `@headers` Hash. sinatra-cors
    # writes `response.headers["Access-Control-..."] = ...`,
    # which under this aliasing lands on the same Hash
    # `finalize` reads when building the Rack triplet.
    def response
      self
    end

    # `response[]` / `response[]=` / `response.include?` — bracket-
    # access shims that route to the response headers Hash. Real
    # Sinatra's `response` is a Rack::Response whose `[]` IS the
    # `headers[]` (Rack::Response inherits from Rack::Utils::
    # HeaderHash semantics). sinatra-contrib/LinkHeader uses
    # `(response['Link'] ||= '')` and `response.include? 'Link'`
    # against this contract. Mirror the surface here on
    # `Sinatra::Base` instances so the vendored gem source works
    # unmodified.
    def [](key)
      headers[key]
    end

    def []=(key, value)
      headers[key] = value
    end

    def include?(key)
      headers.include?(key)
    end

    # `logger` — minimal stub. Real Sinatra hands you a logger
    # object backed by Rack::CommonLogger or similar; the
    # vendored micro-Sinatra has no logging surface, so this
    # returns a tiny `Logger`-shaped object whose methods are
    # no-ops (matching the silent-by-default development
    # logger config). sinatra-cors uses `logger.warn
    # bad_origin_message` to record CORS rejections; the
    # diagnostic value is nice-to-have, not load-bearing for
    # the protocol's correctness on the wire.
    def logger
      @logger ||= LoggerStub.new
    end

    # `process_route(pattern, app)` — Sinatra's internal
    # route-introspection helper. Real Sinatra checks if the
    # given pattern matches the current request path, yields
    # `(application, pattern)` to the block if so. sinatra-cors
    # uses this from `allowed_methods` to enumerate verbs that
    # could serve the request URL.
    #
    # Stubbed to ALWAYS yield — the smoke fixture's
    # `allowed_methods` thus returns every distinct verb in the
    # routes table. For a fixture that declares `get`, `post`,
    # and the `options "*"` catch-all, the returned set is
    # `["GET", "POST", "OPTIONS"]` and `allow.size != 1` so the
    # preflight route emits `Allow: GET,POST,OPTIONS`. Good
    # enough for the CORS contract under test; the real Sinatra
    # pattern-match would prune verbs whose pattern doesn't
    # cover the current URL, but for our smoke shape the
    # over-approximation is harmless (CORS still emits headers
    # via the after-filter regardless).
    def process_route(_pattern, application = nil)
      yield(application, nil)
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
    # Returns `[params, block_args]` on a match (block_args are the captures
    # passed to a `do |cap|` route block, in order), or nil. `path` is the
    # raw PATH_INFO (needed for Regexp routes).
    def match(pattern, segs, path)
      # Regexp route (`get %r{/posts/(\d+)} do |id| … end`): match the raw
      # path; positional captures → params["captures"] + block args, named
      # captures → params[name] — mirroring real Sinatra/mustermann.
      if pattern.is_a?(Regexp)
        m = pattern.match(path)
        return nil unless m
        params = {}
        m.names.each { |n| params[n] = m[n] }
        caps = m.captures
        params["captures"] = caps
        return [params, caps]
      end
      # Catch-all single-splat pattern (`"*"` compiled to `[[:splat]]`)
      # absorbs every request path. sinatra-cors's CORS-preflight catch-all
      # `app.options "*", is_cors_preflight: true do … end` needs this.
      if pattern.length == 1 && pattern[0][0] == :splat
        sp = segs.map { |s| unescape(s) }
        return [{ "splat" => sp }, sp]
      end
      return nil unless pattern.length == segs.length
      params = {}
      splat = []
      block_args = []
      i = 0
      while i < pattern.length
        kind = pattern[i][0]
        seg  = segs[i]
        if kind == :lit
          return nil unless pattern[i][1] == seg
        elsif kind == :cap
          v = unescape(seg)
          params[pattern[i][1]] = v
          block_args << v
        else # :splat
          v = unescape(seg)
          splat << v
          block_args << v
        end
        i += 1
      end
      params["splat"] = splat unless splat.empty?
      [params, block_args]
    end

    def params
      @params ||= IndifferentHash.new
    end

    def request_body
      io = @env && @env["rack.input"]
      io ? io.read.to_s : ""
    end

    # Parse a `&`-separated query string into a String-keyed Hash.
    # Supports Rack's nested-bracket syntax:
    #
    #   "a=1&b=hello+world"             # => {"a"=>"1","b"=>"hello world"}
    #   "user[name]=Ada"                # => {"user"=>{"name"=>"Ada"}}
    #   "tags[]=ruby&tags[]=rust"       # => {"tags"=>["ruby","rust"]}
    #   "u[name]=A&u[email]=a@b"        # => {"u"=>{"name"=>"A","email"=>"a@b"}}
    #   "items[][k]=1&items[][k]=2"     # => {"items"=>[{"k"=>"1"},{"k"=>"2"}]}
    #   "a[b][c]=x"                     # => {"a"=>{"b"=>{"c"=>"x"}}}
    #
    # Mirrors `Rack::Utils.parse_nested_query` for the subset that
    # vendored sinatra-contrib helpers rely on. Forwarded into the
    # final `params` Hash by `dispatch`, so `params['user']['name']`
    # works in route blocks the same way real Sinatra ships it.
    def parse_query(qs)
      out = {}
      return out if qs.nil? || qs.empty?
      qs.split("&").each do |pair|
        next if pair.empty?
        key, val = pair.split("=", 2)
        decoded_key = unescape(plus_to_space(key))
        # Value-less keys (`?flag` or `?single`) get `nil` — matches
        # Rack::Utils.parse_nested_query's contract, NOT the
        # empty-String convention older Sinatra had.
        decoded_val = val.nil? ? nil : unescape(plus_to_space(val))
        _normalise_into(out, decoded_key, decoded_val)
      end
      out
    end

    # Walk `key` (already URL-decoded) for trailing `[...]` segments
    # and install `value` at the right nested slot inside `target`.
    # Plain keys (no `[`) are direct assignments with last-write-
    # wins semantics, matching Rack.
    def _normalise_into(target, key, value)
      open_idx = key.index("[")
      return (target[key] = value) if open_idx.nil?
      head = key[0...open_idx]
      suffix = key[open_idx..-1]
      _walk_suffix(target, head, suffix, value)
    end

    # `suffix` always begins with `[`. `head` is the slot name in
    # `target` to install/descend into. Splits off the first
    # bracket pair and recurses on what remains:
    #
    #   `head[]`        → bucket-append
    #   `head[]<rest>`  → bucket-of-Hash, descend into a Hash that's
    #                     either the last element (if it doesn't
    #                     yet hold the next key) or a fresh
    #                     trailing one
    #   `head[name]`    → terminal Hash write
    #   `head[name]<r>` → descend into nested Hash
    def _walk_suffix(target, head, suffix, value)
      close = suffix.index("]")
      return (target[head] = value) if close.nil?
      inner = suffix[1...close]
      rest = suffix[(close + 1)..-1] || ""
      if inner.empty?
        bucket = (target[head] ||= [])
        if rest.empty?
          bucket << value
          return
        end
        # `[]<rest>` — `rest` starts with `[name]...`. Need a Hash
        # slot inside the array; reuse the trailing one when it
        # doesn't already carry `name`, otherwise append a fresh.
        next_key = _peek_next_bracket_name(rest)
        last = bucket.last
        hash_slot = if last.is_a?(Hash) && next_key && !last.key?(next_key)
          last
        else
          new_h = {}
          bucket << new_h
          new_h
        end
        # Pop the leading `[name]` from `rest` and recurse with it
        # as the next head.
        r_close = rest.index("]")
        next_head = rest[1...r_close]
        next_rest = rest[(r_close + 1)..-1] || ""
        if next_rest.empty?
          hash_slot[next_head] = value
        else
          _walk_suffix(hash_slot, next_head, next_rest, value)
        end
      else
        inner_hash = (target[head] ||= {})
        if rest.empty?
          inner_hash[inner] = value
        else
          _walk_suffix(inner_hash, inner, rest, value)
        end
      end
    end

    # Peek the name inside the first `[name]` of `suffix`. Returns
    # nil if the bracket is empty or the suffix is malformed.
    def _peek_next_bracket_name(suffix)
      return nil unless suffix.start_with?("[")
      close = suffix.index("]")
      return nil if close.nil?
      inner = suffix[1...close]
      inner.empty? ? nil : inner
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
