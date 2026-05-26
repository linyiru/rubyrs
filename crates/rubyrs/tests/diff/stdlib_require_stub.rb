# Tier 1 lenient `require` stub for known pure-Ruby stdlib
# names. `require 'uri'` etc. return `true` without
# actually loading anything — scripts that only require
# stdlib for feature detection (or as no-op dependencies
# of larger files like `gemspec.rb`) proceed past the
# require line. Actual use of the stubbed-out stdlib
# (`URI.parse "..."`, `Logger.new`) fails later with
# NameError, which is the right surface for "feature
# absent in the embedded runtime."
#
# Per ADR 0017, stdlib is Tier 3; this stub is the
# Tier 1 lenient-mode bridge that lets gem helpers load
# their dependency tree without the embedder having to
# vendor pure-Ruby stdlib alongside.
#
# Documented divergences NOT exercised here:
#   - The stubbed module's API isn't actually loaded.
#     `URI.parse "..."` raises NoMethodError in rubyrs
#     vs returning a URI::HTTPS instance in CRuby — the
#     deliberate Tier 1 "feature absent" surface. Fixture
#     stays off that path.

# Each one returns `true` (matching CRuby on first load).
puts require('uri')
puts require('logger')
puts require('json')
puts require('set')
puts require('forwardable')
puts require('singleton')
puts require('delegate')
puts require('pathname')
puts require('digest')
puts require('digest/sha1')
puts require('securerandom')
puts require('yaml')
# `date` is transitively loaded by `yaml` in CRuby, so its
# explicit `require 'date'` returns `false` (already-
# loaded). rubyrs tracks per-name stub state too, but
# doesn't model the transitive-load chain — date would
# return `true` here. Fixture skips `date` to keep
# byte-identical comparison.
puts require('time')
puts require('English')
puts require('erb')

# `require` returning truthy lets the next statement run.
puts "post-require: ok"

# Loaded-features dedup: second require of an already-
# stubbed name returns `false` (matches CRuby).
puts require('uri')         # false — already loaded
puts require('logger')      # false
puts require('json')        # false
# Fresh stub still returns true the first time, false
# every time after. `weakref` is a small stdlib name
# that nothing earlier in the fixture transitively
# loads, so the first-load returns true and the
# second false on both implementations.
puts require('weakref')     # true — first load
puts require('weakref')     # false — re-require

# Stub now materialises the conventional top-level
# constant for each stdlib name, so feature-detection
# patterns like `defined?(URI)` work after require. The
# shell is empty (no methods); calls into it still fail
# with NoMethodError, but the "name exists" surface is
# now correct.
puts defined?(URI)              # "constant"
puts defined?(Logger)           # "constant"
puts defined?(JSON)             # "constant"
puts defined?(SecureRandom)     # "constant"
puts defined?(Pathname)         # "constant"
puts defined?(Forwardable)      # "constant"

# `.name` reads the right string (works for both Module
# and Class in CRuby; rubyrs models them as Class only).
puts URI.name
puts Logger.name
puts JSON.name

# Constants not in the loaded set stay undefined.
puts defined?(NonExistentStdlibConstant)   # nil → blank line
