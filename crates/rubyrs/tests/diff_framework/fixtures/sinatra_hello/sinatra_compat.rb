# sinatra_compat.rb — the ONLY runtime-aware file in this fixture.
#
# It picks who provides `Sinatra::Base` and wraps the per-runtime
# server boot so that the shared app.rb never branches on runtime.
#
#   * On rubyrs, the `RUBYRS` sentinel constant exists (ADR 0026 v2 /
#     GAP #4, M27 B2). RUBY_ENGINE stays "ruby" for drop-in compat, so
#     we use `defined?(RUBYRS)` instead.
#   * On CRuby we load the genuine `sinatra` gem.
#
# Exports:
#   SERVER_BACKEND     — human-readable backend tag (used by the `/`
#                        route in app.rb for transcript identification)
#   HARNESS_RUN_APP    — Proc taking the App class; boots the
#                        per-runtime server on HARNESS_PORT with a
#                        HARNESS_SECS-bounded lifetime so the test
#                        harness's spawn/probe/kill cycle has a known
#                        upper bound.

HARNESS_PORT = (ENV["HARNESS_PORT"] || "9292").to_i
HARNESS_SECS = (ENV["HARNESS_SECS"] || "15").to_i

if defined?(RUBYRS)
  # ---- rubyrs path: vendored micro-Sinatra on the _http_server battery ----
  require_relative "vendor/sinatra_lite"
  SERVER_BACKEND = "rubyrs micro-Sinatra (_http_server battery)"
  HARNESS_RUN_APP = ->(app_class) {
    app_class.run!(bind: "127.0.0.1", port: HARNESS_PORT, duration: HARNESS_SECS)
  }
else
  # ---- CRuby path: the real Sinatra gem ----
  require "sinatra/base"
  SERVER_BACKEND = "CRuby + Sinatra #{Sinatra::VERSION}"

  # One tiny shim so the shared app reads a POST body the same way on
  # both runtimes. (Real Sinatra exposes the body via `request.body`;
  # the rubyrs micro-Sinatra reads env["rack.input"].)
  class Sinatra::Base
    def request_body
      request.body.read
    end
  end

  HARNESS_RUN_APP = ->(app_class) {
    app_class.set :bind, "127.0.0.1"
    app_class.set :port, HARNESS_PORT
    # Sinatra has no per-call duration knob; spawn a daemon thread that
    # calls App.quit! once the harness window elapses. The harness
    # itself also issues child.kill() after probing, so this is a
    # belt-and-braces safety net for runaway tests.
    Thread.new {
      sleep HARNESS_SECS
      app_class.quit!
    }
    app_class.run!
  }
end
