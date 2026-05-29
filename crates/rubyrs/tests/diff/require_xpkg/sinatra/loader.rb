# Canonical gem-entry-point shape: opt into the
# co-located source tree via $LOAD_PATH. Both CRuby and
# rubyrs (since pass-10 layer #6 / PR #295) walk
# `$LOAD_PATH` only — caller-relative resolution belongs
# to `require_relative`. Without these unshift calls,
# `require 'helpers'` / `require 'rack/...'` below would
# LoadError on both runtimes.
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
