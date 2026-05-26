# `Module#constants` walks `include`'d modules' constants
# tables in addition to the receiver's own. Pre-fix
# rubyrs returned only the directly-defined names —
# documented divergence from `module_introspection.rb`'s
# header. Now closed.
#
# Constants are stored under their fully-qualified
# dual-write key (`Foo::BAR` per PR #89). The arm scans
# `Vm.constants` for entries matching the receiver's own
# `"Name::"` prefix AND each included module's prefix,
# de-duping by short name. Sorted lex for stable output.
#
# Documented behaviour (NOT a divergence): the boolean
# `false` arg is accepted but rubyrs always walks
# includes regardless — the distinction (CRuby
# `false` excludes only inherited-from-superclass, NOT
# included-modules) rarely matters and would require
# threading a flag through the include walk.

module Greetings
  GREETING = "hello"
  FAREWELL = "goodbye"
end

module Metrics
  COUNTER = 0
  GAUGE = 1
end

class Hub
  include Greetings
  include Metrics

  HUB_VERSION = "1.0"
end

# Hub's own + both included modules' constants — sorted.
puts Hub.constants.sort.inspect

# Module's own constants still work (no inclusion).
puts Greetings.constants.sort.inspect
puts Metrics.constants.sort.inspect

# Inclusion order doesn't double-count.
class Second
  include Greetings
  include Greetings   # idempotent; rubyrs's include chain
                      # already prevents the dup
  COUNT = 42
end
puts Second.constants.sort.inspect

# Conditional read — `defined?(GREETING)` from outside is
# `nil` because Greetings is at top-level but the constant
# lives prefixed under `Greetings::GREETING`. The arm's
# `&& !short.contains("::")` filter prevents promoting it
# to a top-level `GREETING` answer for `Object.constants`-
# style scans (we don't probe that here).
puts Hub.constants.include?(:GREETING)
puts Hub.constants.include?(:COUNTER)
puts Hub.constants.include?(:HUB_VERSION)

# Element type is Symbol.
puts Hub.constants.first.class
