# sinatra-jsonp consumer app — exercises the gem's `jsonp`
# helper in three shapes: explicit callback arg, callback via
# query param, and no-callback (falls back to bare JSON).
# Same app.rb runs on both runtimes; the only thing that
# changes between runs is who provides `Sinatra::Jsonp#jsonp`
# (vendored source on rubyrs, vendored source via real
# sinatra/base on CRuby — see compat for the loader).

require_relative "sinatra_compat"

class App < Sinatra::Base
  set :environment, :production

  # Modular Sinatra (`class App < Sinatra::Base`) needs to opt
  # into Jsonp explicitly. The gem's `Sinatra.helpers Jsonp` at
  # module level registers on `Sinatra::Application` (CRuby) /
  # `Sinatra::Base` (rubyrs shim) — neither of which our App
  # inherits from automatically in the modular shape. Real
  # third-party-plugin docs uniformly tell modular users to do
  # this explicit `helpers Sinatra::PluginName` line; we follow
  # the documented pattern. The classic Sinatra style
  # (`require 'sinatra'`; bare top-level routes) DOES inherit
  # from Application and skips this line.
  helpers Sinatra::Jsonp

  get "/" do
    "backend: #{SERVER_BACKEND}\nplugin: sinatra-jsonp vendored\n"
  end

  # Explicit callback as second arg to `jsonp`. Hardcodes the
  # callback name so the response is "fn({...})" deterministic.
  get "/data/explicit" do
    jsonp({name: "alice", score: 42}, "myCallback")
  end

  # Auto-pick callback from the query-string `callback=` param.
  # The plugin tries 'callback' / 'jscallback' / 'jsonp' /
  # 'jsoncallback' in order. Verify the first wins.
  get "/data/auto" do
    jsonp({name: "bob", score: 99})
  end

  # No callback supplied AND no callback param — the plugin
  # falls through to bare JSON (content_type :json, just the
  # serialised hash). Different status / shape from the JSONP
  # cases above so a regression in the no-callback branch
  # surfaces.
  get "/data/bare" do
    jsonp({mode: "bare", count: 3})
  end

  # Callback-name sanitisation — the plugin runs
  # `callback.tr!('^a-zA-Z0-9_$\.', '')` to strip anything
  # outside the safe identifier set. Verify with an attempted
  # injection.
  get "/data/sanitised" do
    jsonp({ok: true}, "<script>evil</script>")
  end
end

HARNESS_RUN_APP.call(App)
