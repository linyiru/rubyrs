# sinatra_compat.rb — the ONLY runtime-aware file in this PoC.
#
# It picks who provides `Sinatra::Base`:
#
#   * On rubyrs, the host fn `__rubyrs_http_serve_with_app` exists (it is
#     registered by the `_http_server` battery — see ADR 0022). CRuby has
#     no such method, so `defined?` is the clean discriminator. We can NOT
#     use RUBY_ENGINE: rubyrs deliberately reports "ruby" for maximum
#     drop-in compatibility.
#
#   * On CRuby we load the genuine `sinatra` gem.
#
# Everything the shared app.rb touches (Sinatra::Base, params, run!,
# request_body, SERVER_BACKEND) is made to look identical from here.

if defined?(__rubyrs_http_serve_with_app)
  # ---- rubyrs path: vendored micro-Sinatra on the _http_server battery ----
  require_relative "vendor/sinatra_lite"
  SERVER_BACKEND = "rubyrs micro-Sinatra (_http_server battery)"
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
end
