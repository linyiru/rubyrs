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
#   - rubyrs returns `true` on EVERY require call to a
#     stubbed name (we don't track loaded-features for
#     stubs). CRuby returns `false` on the second and
#     later require of the same name. Mainstream use
#     never depends on this distinction — `require` is
#     idempotent either way.
#   - The stubbed module/class isn't actually loaded.
#     `defined?(URI)` returns nil in rubyrs vs "constant"
#     in CRuby. Fixture stays off that path.

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
# require would return `false` (already-loaded). rubyrs's
# stub doesn't track loaded-features so returns `true` —
# documented divergence. Fixture skips `date` to keep
# byte-identical comparison.
puts require('time')
puts require('English')
puts require('erb')

# `require` returning truthy lets the next statement run.
puts "post-require: ok"
