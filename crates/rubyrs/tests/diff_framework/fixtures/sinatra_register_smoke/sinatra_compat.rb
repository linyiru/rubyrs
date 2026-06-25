# Runtime-aware loader (same shape as sinatra_hello /
# sinatra_plugin_smoke). Vendored micro-Sinatra is shared with
# sinatra_hello via `require_relative "../sinatra_hello/vendor/
# sinatra_lite"`.

HARNESS_PORT = (ENV["HARNESS_PORT"] || "9294").to_i
HARNESS_SECS = (ENV["HARNESS_SECS"] || "15").to_i

if defined?(RUBYRS)
  require "sinatra/base"
  SERVER_BACKEND = "rubyrs micro-Sinatra (_http_server battery)"
  HARNESS_RUN_APP = ->(app_class) {
    app_class.run!(bind: "127.0.0.1", port: HARNESS_PORT, duration: HARNESS_SECS)
  }
else
  require "sinatra/base"
  SERVER_BACKEND = "CRuby + Sinatra #{Sinatra::VERSION}"

  HARNESS_RUN_APP = ->(app_class) {
    app_class.set :bind, "127.0.0.1"
    app_class.set :port, HARNESS_PORT
    Thread.new {
      sleep HARNESS_SECS
      app_class.quit!
    }
    app_class.run!
  }
end
