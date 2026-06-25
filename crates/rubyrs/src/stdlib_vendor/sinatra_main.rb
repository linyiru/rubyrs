# sinatra/main — classic top-level DSL. `require "sinatra"` (as opposed to
# "sinatra/base") sets up classic mode: a default Sinatra::Application, and
# Sinatra::Delegator mixed into the top-level object so bare `get "/" do …`,
# `set`, `before`, `enable`, `configure`, etc. delegate to Application — the
# canonical `require "sinatra"; get("/"){ "hi" }` style.
#
# Mirrors real Sinatra's sinatra/main.rb. The at_exit auto-run (so `ruby
# app.rb` serves) is gated on Application HAVING routes — a modular app that
# `require "sinatra"` then subclasses Sinatra::Base leaves Application empty,
# so it does NOT spuriously start an empty server (the todo-backend shape).
# `disable :run` opts out (used by in-process tests). run! needs _http_server.

require "sinatra/base"

module Sinatra
  class Application < Base
    set :app_file, $0
    set :run, true
    set :method_override, true   # classic apps enable it by default
  end

  module Delegator
    def self.delegate(*methods)
      methods.each do |method_name|
        define_method(method_name) do |*args, &block|
          ::Sinatra::Delegator.target.send(method_name, *args, &block)
        end
        private method_name
      end
    end

    delegate :get, :patch, :put, :post, :delete, :head, :options,
             :template, :layout, :before, :after, :error, :not_found, :configure,
             :set, :mime_type, :enable, :disable, :use, :development?, :test?,
             :production?, :helpers, :settings, :register

    class << self
      attr_accessor :target
    end

    self.target = Application
  end

  at_exit do
    app = Sinatra::Application
    # routes is a RoutesView (Enumerable, no #empty?); #any? is true once a
    # top-level route has been declared (i.e. classic DSL was actually used).
    if $!.nil? && app.settings_store.fetch(:run, true) && app.routes.any?
      app.run!
    end
  end
end

extend Sinatra::Delegator
