# Runtime-aware loader for the rack-protection fixture. Both
# runtimes resolve the four rack-protection files (base +
# FrameOptions + XSSHeader + PathTraversal) to the same vendored
# source. rubyrs additionally vendors no-op `rack.rb` /
# `rack/utils.rb` / `digest.rb` / `logger.rb` / `uri.rb` stubs
# because the real Ruby stdlib (or `rack` gem) isn't loaded under
# the rubyrs runtime; the three middlewares this fixture
# exercises don't reach any of the omitted symbols.

HARNESS_PORT = (ENV["HARNESS_PORT"] || "9304").to_i
HARNESS_SECS = (ENV["HARNESS_SECS"] || "15").to_i

if defined?(RUBYRS)
  require_relative "../sinatra_hello/vendor/sinatra_lite"
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "rack/protection/frame_options"
  require "rack/protection/xss_header"
  require "rack/protection/path_traversal"
  require "rack/protection/referrer_policy"
  require "rack/protection/ip_spoofing"
  require "rack/protection/strict_transport"
  require "rack/protection/content_security_policy"
  require "rack/request"
  require "rack/protection/http_origin"
  SERVER_BACKEND = "rubyrs micro-Sinatra (_http_server battery)"
  HARNESS_RUN_APP = ->(app_class) {
    app_class.run!(bind: "127.0.0.1", port: HARNESS_PORT, duration: HARNESS_SECS)
  }
else
  require "sinatra/base"
  $LOAD_PATH.unshift File.expand_path("vendor", __dir__)
  require "rack/protection/frame_options"
  require "rack/protection/xss_header"
  require "rack/protection/path_traversal"
  require "rack/protection/referrer_policy"
  require "rack/protection/ip_spoofing"
  require "rack/protection/strict_transport"
  require "rack/protection/content_security_policy"
  require "rack/request"
  require "rack/protection/http_origin"
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
