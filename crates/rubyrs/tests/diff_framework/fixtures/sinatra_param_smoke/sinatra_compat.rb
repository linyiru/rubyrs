# Runtime-aware loader for the sinatra-param vendoring fixture.
# Same shape as the sister fixtures: $LOAD_PATH unshift makes
# the vendored sinatra/param.rb resolve before any installed
# gem version, so both rubyrs and CRuby run identical source.

HARNESS_PORT = (ENV["HARNESS_PORT"] || "9297").to_i
HARNESS_SECS = (ENV["HARNESS_SECS"] || "15").to_i

if defined?(RUBYRS)
  require "sinatra/base"
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "sinatra/param"
  SERVER_BACKEND = "rubyrs micro-Sinatra (_http_server battery)"
  HARNESS_RUN_APP = ->(app_class) {
    app_class.run!(bind: "127.0.0.1", port: HARNESS_PORT, duration: HARNESS_SECS)
  }
else
  require "sinatra/base"
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "sinatra/param"
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
