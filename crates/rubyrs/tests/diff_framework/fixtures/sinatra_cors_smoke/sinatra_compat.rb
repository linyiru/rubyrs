# Runtime-aware loader for the sinatra-cors vendoring fixture.
# Identical shape to `sinatra_jsonp_smoke/sinatra_compat.rb` —
# the vendored gem source loads on both runtimes via
# `$LOAD_PATH.unshift File.expand_path("vendor", __dir__)` so
# both rubyrs and CRuby see the same `sinatra/cors.rb` file.

HARNESS_PORT = (ENV["HARNESS_PORT"] || "9296").to_i
HARNESS_SECS = (ENV["HARNESS_SECS"] || "15").to_i

if defined?(RUBYRS)
  require_relative "../sinatra_hello/vendor/sinatra_lite"
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "sinatra/cors"
  SERVER_BACKEND = "rubyrs micro-Sinatra (_http_server battery)"
  HARNESS_RUN_APP = ->(app_class) {
    app_class.run!(bind: "127.0.0.1", port: HARNESS_PORT, duration: HARNESS_SECS)
  }
else
  require "sinatra/base"
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "sinatra/cors"
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
