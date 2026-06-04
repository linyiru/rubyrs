# required_params_smoke app — exercises real
# sinatra-contrib-4.2.1/lib/sinatra/required_params.rb vendored
# 1:1 under vendor/sinatra/required_params.rb. The gem's last
# line is `helpers RequiredParams`, which forwards to
# Sinatra::Base.helpers and mixes RequiredParams's instance
# methods into every Sinatra::Base subclass — so the test app
# gets `required_params` as a route-block helper without any
# explicit `helpers Sinatra::RequiredParams` line.
#
# Covers the helper's three recursion branches end-to-end:
# scalar Symbol args, Array-of-Symbol args, and Hash-of-key
# args. The Hash form needs Rack's bracket-syntax query parser
# (`user[name]=...` → `params['user'] => {'name' => ...}`);
# sinatra_lite now ships that parser too, so the nested
# scenario is exercised on the rubyrs side as well.

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

  # Nested-Hash key shape: `required_params :user => [:name,
  # :email]` requires `params['user']` to exist AND contain both
  # `name` and `email`. The bracket-syntax query parser
  # (`user[name]=Ada&user[email]=ada@example.com`) lands
  # `{'user' => {'name' => 'Ada', 'email' => 'ada@example.com'}}`
  # in params; the helper then recurses into the inner Hash.
  get "/nested" do
    required_params :user => [:name, :email]
    "ok user.name=#{params['user']['name']} user.email=#{params['user']['email']}"
  end

  get "/" do
    "backend: #{SERVER_BACKEND}"
  end
end

HARNESS_RUN_APP.call(RequiredParamsSmokeApp)
