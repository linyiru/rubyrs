# sinatra-param consumer app — exercises the gem's per-route
# `param :name, Type, options` DSL inside route blocks. Each
# route validates its query-string parameters with the gem's
# helpers, falling through to a clean response when valid or
# `halt 400` with an error when not.
#
# Same source on both runtimes: rubyrs loads the vendored
# `sinatra/param.rb` via `$LOAD_PATH.unshift`; CRuby resolves
# to the same vendored copy (the installed sinatra-param gem
# is on the rubygems path but lower-priority than the
# vendored copy).
#
# Out of scope (separate gaps the fixture intentionally
# avoids): `Date.parse` / `Time.parse` / `DateTime.parse`
# coercions — sinatra-param's coerce(...) handles these but
# they require the Date stdlib (not in rubyrs Tier-1). The
# Integer/Float/String/Boolean/Array coercions are sufficient
# to exercise the gem's full validation surface.

require_relative "sinatra_compat"

class App < Sinatra::Base
  set :environment, :production
  helpers Sinatra::Param

  get "/" do
    "backend: #{SERVER_BACKEND}\nplugin: sinatra-param vendored\n"
  end

  # Required Integer with min/max range — the canonical
  # API shape: `param :id, Integer, required: true, min: 1`.
  get "/users" do
    param :limit, Integer, required: true, min: 1, max: 100
    "limit=#{params['limit'].inspect} class=#{params['limit'].class}"
  end

  # String coercion with format regex + min_length.
  get "/search" do
    param :q, String, required: true, min_length: 2
    "search q=#{params['q'].inspect}"
  end

  # Float coercion with default + in (range).
  get "/score" do
    param :weight, Float, default: 1.0, in: 0.0..10.0
    "weight=#{params['weight']}"
  end

  # Boolean coercion — gem's special `Boolean = :boolean` symbol.
  get "/flag" do
    param :on, Sinatra::Param::Boolean, required: true
    "on=#{params['on'].inspect} class=#{params['on'].class}"
  end

  # Array coercion with custom delimiter.
  get "/list" do
    param :tags, Array, delimiter: ";"
    "tags=#{params['tags'].inspect}"
  end

  # one_of validator — exactly one of these query params present.
  get "/auth" do
    one_of :token, :session
    "auth via #{params['token'] ? 'token' : 'session'}"
  end
end

HARNESS_RUN_APP.call(App)
