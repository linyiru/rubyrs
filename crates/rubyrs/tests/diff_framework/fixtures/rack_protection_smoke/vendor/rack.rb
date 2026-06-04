# `require 'rack'` umbrella stub. The real `rack` gem reopens
# this module to declare the Rack::Builder / Rack::Request /
# etc. surface; for the rack_protection_smoke fixture we don't
# need any of it because we vendor the three middlewares
# directly and `use` them through sinatra_lite's existing
# middleware chain. The base.rb file declares `require 'rack'`
# at its top — this empty module declaration is enough to satisfy
# the require without forcing real-Rack dependency on rubyrs.

module Rack
end
