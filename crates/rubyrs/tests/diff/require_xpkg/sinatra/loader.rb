# Sibling require (require 'helpers' finds ./helpers.rb).
require 'helpers'

# Cross-package require: caller is in .../sinatra/, target
# is .../rack/X.rb — resolves via caller-parent search.
require 'rack/show_exceptions'
require 'rack/utils'

# Cross-package require to a sibling-of-sinatra package.
require 'common/log'

puts Sinatra::Helpers.greet
puts Rack::ShowExceptions.from_rack
puts Rack::Utils.escape("a b")
Common::Log.write("logged")
