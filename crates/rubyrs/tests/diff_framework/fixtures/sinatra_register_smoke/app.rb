# Real-shape Sinatra app using the canonical `register` /
# `helpers` plugin authoring pattern. The application code
# below is what a user would write with any real third-party
# plugin (sinatra-cors, sinatra-flash, etc.); the plugin file
# itself uses `app.helpers` + `app.before` + `app.get` inside
# its `self.registered(app)` entry point.

require_relative "sinatra_compat"
require_relative "plugin"

class App < Sinatra::Base
  set :environment, :production

  # Canonical plugin install line. Real Sinatra apps look
  # exactly like this with `register Sinatra::Cors`,
  # `register Sinatra::Flash`, etc.
  register Sinatra::MetaInfo

  get "/" do
    "backend: #{SERVER_BACKEND}\n" \
      "plugin: #{meta_info_string}\n" \
      "count: #{meta_info_request_count}\n"
  end

  # Helper visible from app routes — proves `helpers Mod` mixed
  # the module's instance methods into App.
  get "/user/:name" do
    "hello #{params['name']}, #{meta_info_string}"
  end

  # Before-filter side effect — @meta_info_seeded was set by
  # the plugin's before-filter on every dispatch. If the filter
  # didn't run, the ivar would be nil and the substring below
  # would print "marked: " — the trailing `=true` is the
  # filter-fired signal.
  get "/marked" do
    "marked: seeded=#{@meta_info_seeded.inspect}"
  end

  # Constant access — proves `Sinatra::MetaInfo::VERSION`
  # crosses the require boundary into the app's lexical scope.
  get "/plugin/version" do
    Sinatra::MetaInfo::VERSION
  end

  # Note: the plugin file ALSO installed a `/__meta` route via
  # `app.get "/__meta" do ...; end` inside `registered(app)`.
  # That route resolves through the same app routing table as
  # the routes defined above — proving plugin-added routes
  # work the same way app-defined routes do.
end

HARNESS_RUN_APP.call(App)
