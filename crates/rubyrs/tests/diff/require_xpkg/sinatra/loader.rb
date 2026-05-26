# Canonical gem-entry-point shape: opt into the
# co-located source tree via $LOAD_PATH so both CRuby
# (which only walks $LOAD_PATH) and rubyrs (which also
# consults caller-relative paths) resolve the same files.
#
# `__dir__` is the sinatra/ directory; `..` of that is the
# package root containing sinatra/, rack/, common/.
$LOAD_PATH.unshift __dir__
$LOAD_PATH.unshift File.expand_path("..", __dir__)

# Sibling require — `require 'helpers'` finds helpers.rb
# next to this file.
require 'helpers'

# Cross-package requires — `rack/...` and `common/...`
# resolve via the package root added above.
require 'rack/show_exceptions'
require 'rack/utils'
require 'common/log'

puts Sinatra::Helpers.greet
puts Rack::ShowExceptions.from_rack
puts Rack::Utils.escape("a b")
Common::Log.write("logged")
