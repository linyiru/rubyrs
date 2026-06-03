# sinatra-cors consumer app — exercises the vendored gem's
# `register Sinatra::Cors` install + `set :allow_origin, ...`
# / `allow_methods, ...` / `allow_headers, ...` configuration +
# the resulting CORS headers on responses.
#
# Same app.rb runs on both runtimes; the only thing that
# changes is who provides `Sinatra::Cors` (vendored source
# loaded via `require "sinatra/cors"` ahead of any rubygems-
# resolved version, see `sinatra_compat.rb` for the
# `$LOAD_PATH.unshift` setup).

require_relative "sinatra_compat"

class App < Sinatra::Base
  set :environment, :production

  register Sinatra::Cors

  # sinatra-cors splits these on `,` (the `/\s*,\s*/` regex);
  # values must be comma-separated even though Sinatra's docs
  # sometimes show space-separated form in examples.
  set :allow_origin,  "http://example.com"
  set :allow_methods, "GET, POST"
  set :allow_headers, "content-type"

  get "/" do
    "backend: #{SERVER_BACKEND}\nplugin: sinatra-cors vendored\n"
  end

  get "/data" do
    "data-payload"
  end

  post "/data" do
    "data-posted"
  end
end

HARNESS_RUN_APP.call(App)
