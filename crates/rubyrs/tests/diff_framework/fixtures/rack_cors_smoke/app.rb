# rack_cors_smoke app — exercises real rack-cors-3.0.0 middleware
# vendored 1:1 under vendor/rack/cors.rb. The fixture proves the
# `use Klass do ... end` middleware path works end-to-end with the
# unmodified gem source on both rubyrs and CRuby.

require_relative "sinatra_compat"

class RackCorsSmokeApp < Sinatra::Base
  use Rack::Cors do
    # `origins '*'` triggers `@public_resources = true`, skipping
    # the Regexp matcher and emitting `Access-Control-Allow-Origin: *`.
    allow do
      origins "*"
      resource "/public/*", headers: :any, methods: [:get, :options]
    end

    # Specific-origin form — origins '...' converts to
    # Regexp.compile("^[a-z]+://#{Regexp.quote(...)}$"), matched via
    # `source =~ origin`. Exercises the Step-1 Regexp class methods.
    allow do
      origins "example.com"
      resource "/api/*", headers: :any, methods: %i[get post options]
    end
  end

  get "/" do
    "rack-cors backend: #{SERVER_BACKEND}"
  end

  get "/public/info" do
    "public info"
  end

  get "/api/users" do
    "api users"
  end
end

HARNESS_RUN_APP.call(RackCorsSmokeApp)
