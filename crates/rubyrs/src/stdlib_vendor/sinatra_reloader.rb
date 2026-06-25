# sinatra/reloader — no-op shim of sinatra-contrib's Reloader.
#
# The real Reloader is a DEVELOPMENT-only code reloader (it watches source
# files and re-evaluates changed routes between requests). In production it
# is inert, and Sinatra's own docs gate it with `require "sinatra/reloader"
# if development?`. rubyrs ships a no-op so the common
#
#     configure :development do
#       require "sinatra/reloader"
#       register Sinatra::Reloader
#       also_reload "lib/**/*.rb"
#     end
#
# pattern LOADS and is inert with zero code change — production-correct, and
# in development it simply doesn't hot-reload (the script is re-run instead).
# Actual file-watching reloading would need a dev server loop; out of scope.

require "sinatra/base"

module Sinatra
  module Reloader
    # `register Sinatra::Reloader` extends the app with these class methods
    # so `also_reload` / `dont_reload` calls resolve (and do nothing).
    module ClassMethods
      def also_reload(*); end
      def dont_reload(*); end
    end

    def self.registered(app)
      app.extend(ClassMethods)
    end
  end
end
