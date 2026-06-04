# Runtime-aware loader for the sinatra-contrib/JSON vendoring
# fixture. Same shape as multi_route_smoke / required_params_smoke:
# both runtimes resolve `sinatra/json` to the same vendored source.

HARNESS_PORT = (ENV["HARNESS_PORT"] || "9300").to_i
HARNESS_SECS = (ENV["HARNESS_SECS"] || "15").to_i

if defined?(RUBYRS)
  require_relative "../sinatra_hello/vendor/sinatra_lite"
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "sinatra/json"
  SERVER_BACKEND = "rubyrs micro-Sinatra (_http_server battery)"
  HARNESS_RUN_APP = ->(app_class) {
    app_class.run!(bind: "127.0.0.1", port: HARNESS_PORT, duration: HARNESS_SECS)
  }
else
  require "sinatra/base"
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "sinatra/json"
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
