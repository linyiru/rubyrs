# `END { }` desugars to `at_exit { }` (same LIFO-at-exit contract);
# `BEGIN { }` runs inline at its position (Tier-1: conventional
# top-of-file placement, not hoisted). They interleave with explicit
# at_exit in one LIFO stack.

BEGIN { puts "begin-1" }
END { puts "end-1" }
END { puts "end-2" }
at_exit { puts "atexit-1" }
BEGIN { puts "begin-2" }

puts "main line"

at_exit { puts "atexit-2" }
END { puts "end-3" }
