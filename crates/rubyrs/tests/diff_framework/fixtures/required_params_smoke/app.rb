# required_params_smoke app — exercises real
# sinatra-contrib-4.2.1/lib/sinatra/required_params.rb vendored
# 1:1 under vendor/sinatra/required_params.rb. The gem's last
# line is `helpers RequiredParams`, which forwards to
# Sinatra::Base.helpers and mixes RequiredParams's instance
# methods into every Sinatra::Base subclass — so the test app
# gets `required_params` as a route-block helper without any
# explicit `helpers Sinatra::RequiredParams` line.
#
# Nested-Hash key shapes (e.g. `required_params :user => [:name]`)
# require Rack's bracket-syntax query parser (`user[name]=...`
# → `params['user'] => {'name' => ...}`). Our sinatra_lite ships
# with flat string-keyed params; the nested shape is exercised
# end-to-end by real Rack-backed integration tests in CRuby
# downstream, not here. This fixture covers the simple-keys and
# Array-form recursion branches — the two branches the helper-
# install + halt path most directly verifies.

require_relative "sinatra_compat"

class RequiredParamsSmokeApp < Sinatra::Base
  # Real Sinatra's modular form requires the explicit
  # `helpers Sinatra::RequiredParams` line — the gem's module-
  # level `helpers RequiredParams` at the end of
  # `required_params.rb` installs onto the default classic
  # application class, not onto every Sinatra::Base subclass.
  # rubyrs's sinatra_lite happens to install onto Sinatra::Base
  # at the module-level forward, so this line is a no-op on
  # rubyrs but load-bearing on CRuby; the explicit shape keeps
  # both runtimes' parity identical.
  helpers Sinatra::RequiredParams

  # Simple-keys shape.
  get "/simple" do
    required_params :a, :b
    "ok a=#{params['a']} b=#{params['b']}"
  end

  # Array form: `required_params [:x, :y]` is the same as
  # `required_params :x, :y`. Exercises the Array-recurse
  # branch of the helper's pattern-match.
  get "/array_form" do
    required_params [:x, :y]
    "ok x=#{params['x']} y=#{params['y']}"
  end

  # Mixed shape: a Symbol plus a Symbol-keyed Array. Hits both
  # the scalar branch (`:a`) and the Array branch (`[:b, :c]`)
  # in the same call, verifying the recursion threads cleanly.
  get "/mixed" do
    required_params :a, [:b, :c]
    "ok a=#{params['a']} b=#{params['b']} c=#{params['c']}"
  end

  get "/" do
    "backend: #{SERVER_BACKEND}"
  end
end

HARNESS_RUN_APP.call(RequiredParamsSmokeApp)
