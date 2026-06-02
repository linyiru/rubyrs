# `Array.new(N) { |i| block }` — the block-form constructor
# CRuby has always supported. rubyrs previously returned the
# generic Class.new shell (`#<Array>`) for this shape; surfaced
# by the SQLite bench's `LOOKUP_IDS = Array.new(ITERS) { … }`
# pattern. Fixed in vm/dispatch.rs do_call_block's Array.new
# intercept.

# Basic — Range mapping.
p Array.new(3) { |i| i * 2 }
p Array.new(5) { |i| i + 1 }

# Zero-size returns empty array.
p Array.new(0) { |i| fail "shouldn't run" }

# Block return value is whatever element type — strings, symbols.
p Array.new(4) { |i| "s#{i}" }
p Array.new(3) { |i| (97 + i).chr.to_sym }

# Block can ignore its arg.
counter = 0
result = Array.new(5) { counter += 1 }
p result

# `break val` from inside the block returns val as the call's
# overall result (not the partial Array). CRuby semantics.
r = Array.new(10) { |i| break :stop if i == 3; i }
p r
