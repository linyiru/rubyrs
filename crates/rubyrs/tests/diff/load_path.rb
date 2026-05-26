# `$LOAD_PATH` / `$:` — array of require search paths.
# Previously `nil` in rubyrs; now a real Array that scripts
# can mutate. `$:` is an alias for the same Array (CRuby
# semantics).
#
# Documented divergence NOT covered: CRuby pre-populates
# `$LOAD_PATH` with stdlib paths (~9 entries from the Ruby
# install + rubygems); rubyrs starts with an empty Array
# and lets the script populate it. The fixture stays off
# the initial-content path (counts, indexed read) and
# probes only the script-driven mutations both
# implementations agree on.

puts $LOAD_PATH.class                # Array
puts $LOAD_PATH.is_a?(Array)         # true

# Identity: $: is the same Array.
puts $:.equal?($LOAD_PATH)           # true

# Mutate via unshift — entries land at the front in
# reverse-call order.
before_len = $LOAD_PATH.length
$LOAD_PATH.unshift "/tmp/probe-1"
$LOAD_PATH.unshift "/tmp/probe-2"
puts $LOAD_PATH.length - before_len  # 2
puts $LOAD_PATH[0]                   # "/tmp/probe-2"
puts $LOAD_PATH[1]                   # "/tmp/probe-1"

# Variadic unshift puts args in order at front.
before_len = $LOAD_PATH.length
$LOAD_PATH.unshift "/a", "/b", "/c"
puts $LOAD_PATH.length - before_len  # 3
puts $LOAD_PATH[0]                   # "/a"
puts $LOAD_PATH[1]                   # "/b"
puts $LOAD_PATH[2]                   # "/c"

# `prepend` is an alias for unshift.
$LOAD_PATH.prepend "/zzz"
puts $LOAD_PATH[0]                   # "/zzz"

# Mutating via `$:` writes the same Array.
$:.unshift "/via-shortname"
puts $LOAD_PATH[0]                   # "/via-shortname"
