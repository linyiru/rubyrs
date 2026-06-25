# sinatra_compat.rb — runtime-aware loader. Same shape as the
# sister fixture `sinatra_hello/sinatra_compat.rb`; documented in
# detail there.
#
# This fixture deliberately SHARES the vendored micro-Sinatra
# with sinatra_hello (`require_relative "../sinatra_hello/vendor/
# sinatra_lite"`) so any improvement to the vendored runtime
# benefits both fixtures at once and there's only one source of
# truth. Path is fixture-relative (sibling lookup under the same
# `fixtures/` dir) and works on both runtimes — `require_relative`
# resolves against the script's `__FILE__`.

HARNESS_PORT = (ENV["HARNESS_PORT"] || "9293").to_i
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
