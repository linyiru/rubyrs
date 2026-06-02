# Runtime-aware bootstrap for sinatra-jsonp vendored fixture.
# On rubyrs: loads micro-Sinatra (shared with sinatra_hello)
# + the local multi_json shim + the verbatim-vendored
# sinatra/jsonp.rb. On CRuby: loads the real `sinatra/base`
# gem + the real `multi_json` gem + the verbatim-vendored
# sinatra/jsonp.rb (so the exact-same plugin source loads on
# both runtimes — that's the point of the fixture). The
# vendored-jsonp file's `require 'multi_json'` resolves to
# rubyrs's local shim on rubyrs, to the installed gem on
# CRuby, transparently.

HARNESS_PORT = (ENV["HARNESS_PORT"] || "9295").to_i
HARNESS_SECS = (ENV["HARNESS_SECS"] || "15").to_i

if defined?(RUBYRS)
  require_relative "../sinatra_hello/vendor/sinatra_lite"
  # Put this fixture's vendor/ on the load path so the
  # vendored sinatra/jsonp.rb's `require 'multi_json'` finds
  # the shim, and so `require 'sinatra/jsonp'` resolves to
  # the vendored file rather than the (absent) gem.
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "sinatra/jsonp"
  SERVER_BACKEND = "rubyrs micro-Sinatra (_http_server battery)"
  HARNESS_RUN_APP = ->(app_class) {
    app_class.run!(bind: "127.0.0.1", port: HARNESS_PORT, duration: HARNESS_SECS)
  }
else
  require "sinatra/base"
  # The vendored file lives at vendor/sinatra/jsonp.rb. Adding
  # vendor/ to $LOAD_PATH makes `require 'sinatra/jsonp'`
  # resolve here BEFORE the installed gem (LoadError if gem is
  # absent — but rubygems would have served it; we want the
  # vendored copy to load so both runtimes run identical
  # plugin source).
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "sinatra/jsonp"
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
