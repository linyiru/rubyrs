# multi_route_smoke app — exercises real
# sinatra-contrib-4.2.1/lib/sinatra/multi_route.rb vendored 1:1
# under vendor/sinatra/multi_route.rb. Proves
# `Class#extend(Mod)` (singleton_includes fix), class-method
# `super` through extended modules, and
# `super(*args, &block)` (Op::ApplySuperBlock) all stack up on
# both rubyrs and CRuby with the unmodified gem source.

require_relative "sinatra_compat"

class MultiRouteSmokeApp < Sinatra::Base
  register Sinatra::MultiRoute

  # Multi-path single-verb: MultiRoute overrides `get` to route
  # each entry in the array list through `super(verb, path,
  # opts, &block)` after demuxing.
  get "/multi_a", "/multi_b" do
    "multi paths: #{request.path}"
  end

  # Multi-verb single-path: `route :get, :post, '/v'` registers
  # the SAME block under both verbs. Verifies the explicit
  # `route` entry point on sinatra_lite.
  route :get, :post, "/verbs" do
    "verbs path: #{request.request_method}"
  end

  # Multi-verb multi-path: registers cartesian product.
  route :get, :post, ["/cart_x", "/cart_y"] do
    "#{request.request_method} #{request.path}"
  end

  # Baseline single-path single-verb still works (MultiRoute's
  # override transparently routes single-arg through super).
  get "/single" do
    "single"
  end

  get "/" do
    "multi-route backend: #{SERVER_BACKEND}"
  end
end

HARNESS_RUN_APP.call(MultiRouteSmokeApp)
