# BigInt Phase B.1 — `**` exponentiation with auto-promote.
#
# Pre-Phase-B, Int#** used saturating_pow and capped at i64::MAX;
# `2 ** 100` returned 9_223_372_036_854_775_807 instead of the
# real 1.27e30. Now: numeric_call declines on overflow, do_call
# routes through Vm::bigint_primitive which calls Vm::try_bigint_pow
# — the helper estimates result size, traps ResourceExhausted if
# it would blow up, and computes the precise BigInt result
# otherwise.

# Int × Int small-exp result still fits i64 (fast path, no
# allocation).
puts 3 ** 30
puts 5 ** 5

# Int × Int that overflows i64 → BigInt
puts 2 ** 63
puts 2 ** 100
puts 10 ** 20

# Identity / boundary cases
puts 0 ** 0
puts 0 ** 5
puts 5 ** 0
puts 1 ** 100
puts 1 ** 100_000  # 1**huge fits trivially (bits ≤ 1 short-circuit)

# Negative exponent → Float (documented Rational divergence
# — see SUBSET.md; CRuby returns Rational `(1/4)`, we return
# `0.25`. Skipped from this byte-exact fixture; see
# `tests/embed.rs` for the assertion that the Float path runs.)

# BigInt receiver ** Int — squaring a BigInt
big = 2 ** 100
puts big ** 2
puts big ** 0
puts big ** 1

# Operator method-call shape — `n.**(exp)`, `n.send(:**, exp)`.
# Must match Op::BinOp form exactly.
puts 2.**(100)
puts 2.send(:**, 100)
puts 2.send(:**, 63)
