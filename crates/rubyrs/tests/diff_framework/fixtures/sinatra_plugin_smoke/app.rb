# app.rb — exercises the `plugin.rb` helpers from inside a normal
# Sinatra application class. The plugin lives in a separate file
# that's `require_relative`d, exactly as a third-party gem would
# arrive via `require "sinatra/greet_plugin"`.

require_relative "sinatra_compat"
require_relative "plugin"

class App < Sinatra::Base
  set :environment, :production

  get "/" do
    "backend: #{SERVER_BACKEND}\n#{greet_plugin_info}\n"
  end

  # One route per generated helper — proves the loop captured
  # `style` correctly. Different output bodies between routes
  # would mean only the LAST iteration's style stuck (the classic
  # define_method-without-block-capture bug).
  get "/greet/formal/:name" do
    greet_plugin_formal(params["name"])
  end

  get "/greet/casual/:name" do
    greet_plugin_casual(params["name"])
  end

  get "/greet/friendly/:name" do
    greet_plugin_friendly(params["name"])
  end

  # Direct call to the vanilla reopened-Base method.
  get "/plugin/info" do
    greet_plugin_info
  end

  # Constant access from inside a route block.
  get "/plugin/version" do
    SinatraGreetPlugin::VERSION
  end
end

HARNESS_RUN_APP.call(App)
