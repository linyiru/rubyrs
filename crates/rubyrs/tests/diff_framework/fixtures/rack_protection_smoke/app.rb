# rack_protection_smoke app — exercises three real
# rack-protection-4.2.1 middlewares vendored 1:1: FrameOptions,
# XSSHeader, PathTraversal. The subset is the "security headers
# only" slice of the gem — no CSRF token state, no session
# hijacking detection, no IP spoofing checks (those would need
# request state we don't model). Covers the three most-used
# protection middlewares all by themselves so the fixture stays
# tractable.

require_relative "sinatra_compat"

class RackProtectionSmokeApp < Sinatra::Base
  use Rack::Protection::FrameOptions
  use Rack::Protection::XSSHeader
  use Rack::Protection::PathTraversal
  use Rack::Protection::ReferrerPolicy
  use Rack::Protection::IPSpoofing
  use Rack::Protection::StrictTransport

  # Routes that exercise the security-header-injection paths.
  get "/" do
    "backend: #{SERVER_BACKEND}"
  end

  # Path-traversal cleanup — the middleware unescapes %2e (.) and
  # %2f (/) BEFORE the route matcher sees it; the app handler
  # then sees the cleaned PATH_INFO. After the request, PATH_INFO
  # is restored to the pre-cleanup value via the `ensure` block
  # so downstream middlewares see the original.
  get "/clean" do
    "you reached /clean"
  end

  # `/echo_path` reports the cleaned PATH_INFO so we can verify
  # cleanup logic from inside a route. PathTraversal rewrites
  # PATH_INFO BEFORE the route matcher sees it, so a request
  # like `/%2e%2e/echo_path/x/%2e%2e` cleans to `/echo_path`
  # (parent-segment pops, dot-segment skips) and hits this
  # route. Routes after a traversal that cleans to a path with
  # no handler would land on Sinatra's 404 shell, whose body
  # differs between environments — keep the scenarios to ones
  # that survive cleanup so the diff is over the middleware's
  # behaviour, not the 404 fallback.
  get "/echo_path" do
    "path=#{request.path}"
  end

  # A second cleanup-target route used for the
  # encoded-slash scenario: `/foo%2fbar` decodes the `%2f` into
  # a real `/` separator and the cleaned PATH_INFO becomes
  # `/echo_path/bar`. Pre-fix (no PathTraversal middleware) the
  # %2f would have stayed encoded.
  get "/echo_path/:tail" do
    "tail=#{params['tail']}"
  end
end

HARNESS_RUN_APP.call(RackProtectionSmokeApp)
