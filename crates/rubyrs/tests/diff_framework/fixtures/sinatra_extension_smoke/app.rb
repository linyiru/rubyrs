# sinatra_extension_smoke app — exercises real
# sinatra-contrib-4.2.1/lib/sinatra/extension.rb vendored 1:1.
# Sinatra::Extension is a method-recorder pattern: a module
# `extend`s it to capture every Sinatra::Base method call, then
# when the user app calls `register MyExt`, Extension's
# `registered(base)` hook replays the recorded calls onto the
# app class.

require_relative "sinatra_compat"

# Define an extension the recorder way. `get` / `before` /
# `helpers` etc. would normally fail at module-scope (the Module
# doesn't define them), but Extension's method_missing fires
# because Sinatra::Base.respond_to?(method) is true — recording
# the call into @recorded_methods for later replay.
module GreetingExt
  extend Sinatra::Extension

  before do
    @greeting = "hello"
  end

  get "/greet/:name" do
    "#{@greeting}, #{params['name']}!"
  end

  get "/extension_marker" do
    "this route came from GreetingExt"
  end
end

# Define a second extension via the block-form `new` — same
# pattern as the gem's docs (`Sinatra::Extension.new { ... }`).
# The block runs against a fresh Module already `extend`-ed with
# Extension, so every call inside `class_eval` lands in the
# recorder.
StatsExt = Sinatra::Extension.new do
  get "/stats/uptime" do
    "uptime=42s"
  end
end

class SinatraExtensionSmokeApp < Sinatra::Base
  register GreetingExt
  register StatsExt

  get "/" do
    "backend: #{SERVER_BACKEND}"
  end
end

HARNESS_RUN_APP.call(SinatraExtensionSmokeApp)
