# app.rb — ONE Sinatra application, byte-for-byte identical, that runs on:
#
#   * CRuby   — backed by the real `sinatra` gem (require "sinatra/base")
#   * rubyrs  — backed by a vendored micro-Sinatra on the `_http_server`
#               battery (no gems, no C-exts, sandbox-friendly)
#
# The only thing that changes between the two is *who provides*
# `Sinatra::Base` and the server loop — selected in sinatra_compat.rb by
# feature-detecting a rubyrs-only host fn. The application code below never
# branches on the runtime.
#
# Run:
#   ruby                poc/sinatra/app.rb              # CRuby + real Sinatra
#   target/debug/rubyrs poc/sinatra/app.rb              # rubyrs micro-Sinatra
#
# See verify.sh for the full route matrix + cross-runtime diff.

require_relative "sinatra_compat"

class App < Sinatra::Base
  # Run in production: real Sinatra defaults to :development under
  # `run!`, where the show_exceptions debug page would intercept errors
  # before `error` handlers and dump a backtrace. Production makes the
  # `error` handlers authoritative (and is how you'd actually deploy).
  set :environment, :production

  # A before-filter — runs in the route's instance context on both.
  before do
    @greeting = "Hello"
  end

  get "/" do
    "<h1>#{@greeting} from #{SERVER_BACKEND}</h1>\n" \
    "<p>Try <a href=\"/hello/world\">/hello/world</a></p>\n"
  end

  # Path parameter.
  get "/hello/:name" do
    "#{@greeting}, #{esc(params['name'])}! (served by #{SERVER_BACKEND})\n"
  end

  # Query-string parameters (URL-decoded, '+' -> space).
  get "/search" do
    q     = params["q"]     || ""
    limit = params["limit"] || "10"
    "results for '#{q}' (limit #{limit})\n"
  end

  # Splat / wildcard segments -> params["splat"].
  get "/say/*/to/*" do
    who, what = params["splat"]
    "#{who} says #{what}\n"
  end

  # halt with an explicit status + body.
  get "/admin" do
    halt 403, "Forbidden\n"
  end

  # redirect (302 + Location header).
  get "/old" do
    redirect "/new"
  end

  # custom status + content type.
  get "/teapot" do
    content_type "text/plain"
    status 418
    "I'm a teapot\n"
  end

  # Reads the request body via rack.input (GAP #2, fixed) — identical to
  # real Sinatra's request.body.read.
  post "/echo" do
    "echo: #{request_body}\n"
  end

  # Form-encoded POST body parsed into params (like an HTML form submit).
  post "/form" do
    "form: name=#{params['name']} city=#{params['city']}\n"
  end

  # The `request` object — read a request header (User-Agent).
  get "/whoami" do
    "ua=#{request.user_agent}\n"
  end

  # Read a cookie via request.cookies.
  get "/prefs" do
    "theme=#{request.cookies['theme'] || 'default'}\n"
  end

  # `pass` — the first matching route bails to the next one.
  get "/feature" do
    pass if params["skip"] == "yes"
    "feature: primary\n"
  end
  get "/feature" do
    "feature: fallback\n"
  end

  # PUT verb + path param.
  put "/resource/:id" do
    "updated resource #{params['id']}\n"
  end

  # A custom exception raised in a route, mapped to an `error` handler.
  class ValidationError < StandardError; end

  error ValidationError do
    status 422
    "validation failed\n"
  end

  get "/validate" do
    raise ValidationError, "bad input"
  end

  # Streaming response. On rubyrs each `out <<` is flushed as its own
  # chunked HTTP/1.1 frame via a Fiber (ADR 0023) — the same source the
  # real Sinatra gem runs through Rack streaming.
  get "/stream" do
    stream do |out|
      i = 1
      while i <= 3
        out << "tick #{i}\n"
        i += 1
      end
    end
  end

  # Custom 404.
  not_found do
    "custom 404 — no such route\n"
  end

  # A plain instance method, visible inside route blocks on both runtimes.
  # (Named `esc`, not `escape`, to avoid colliding with Rack::Utils.escape
  # which real Sinatra mixes in.)
  def esc(str)
    str.to_s.gsub("<", "&lt;").gsub(">", "&gt;")
  end
end

HARNESS_RUN_APP.call(App)
