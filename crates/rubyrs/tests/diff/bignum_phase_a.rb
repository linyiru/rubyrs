# BigInt PoC Phase A — Integer-as-Bignum auto-promote on i64
# overflow, demote-on-fit, mixed Int/BigInt arithmetic +
# comparison. Cfg-gated on the `bignum` feature.
#
# Out of scope (Phase B):
#   - `**` operator (saturating today; use repeated `*` here)
#   - bit ops `& | ^ << >> ~`
#   - Float ↔ BigInt mixed arithmetic
#   - Integer#bit_length, Integer#to_s(base), Integer#digits
#   - Hash key support

# Mul overflow → BigInt
puts 1_000_000_000 * 1_000_000_000 * 1_000_000_000

# Add/Sub overflow
puts 9_000_000_000_000_000_000 + 9_000_000_000_000_000_000
puts(-9_000_000_000_000_000_000 - 9_000_000_000_000_000_000)

# Literal beyond i64 — BigInt at parse time
puts 9_223_372_036_854_775_808
puts 999_999_999_999_999_999_999_999
puts(-999_999_999_999_999_999_999_999)
puts 9_223_372_036_854_775_808.class

# Iterative product — factorial 30 (= 265252859812191058636308480000000)
def fact(n)
  acc = 1
  i = 1
  while i <= n
    acc = acc * i
    i = i + 1
  end
  acc
end
puts fact(20)
puts fact(25)
puts fact(30)

# Demote-on-fit: BigInt that shrinks back to i64 prints as the
# normal Int (no BigInt-vs-Int identity divergence on round trip).
big = 1_000_000_000 * 1_000_000_000 * 1_000_000_000
small = big / (1_000_000_000 * 1_000_000_000)
puts small
puts small.class

# Mixed Int + BigInt — Int coerces up.
puts (1 + fact(25))
puts (fact(25) + 1)
puts (2 * fact(25))

# Subtraction back to Int range
puts (fact(30) - fact(30) + 7)

# Floor-div + mod on BigInt operands (positive — negative-divisor
# Int×Int has a pre-existing rubyrs divergence from CRuby that's
# unrelated to BigInt; covered separately by SUBSET.md).
puts (fact(30) / 7)
puts (fact(30) % 7)

# Comparison: Fixnum-shape equality across Int and BigInt round trip.
puts (fact(20) == 2_432_902_008_176_640_000)
puts (fact(30) > fact(20))
puts (fact(30) > 10)
puts (10 < fact(30))
puts (fact(30) == fact(30))

# `class` reports Integer for both Fixnum-fit and Bignum-overflow.
puts fact(20).class
puts fact(30).class

# Method-call shape — `to_s` and `inspect` on a BigInt receiver
# must work via direct invocation AND via `send`. Pre-PR these
# raised NoMethodError because the BigInt dispatch path was only
# wired into Op::BinOp, not the do_call method-lookup path.
puts fact(30).to_s
puts fact(30).inspect
puts fact(30).send(:to_s)

# Operator method-call shape — `big.+(x)` / `big.send(:==, y)`
# must match the Op::BinOp result exactly. Pre-cycle-2 these
# fell through to ruby_eq's identity arm and returned the wrong
# answer for canonical-value equality.
big = 9_999_999_999_999_999_999
puts big.+(1)
puts big.send(:+, 1)
puts big.send(:==, 9_999_999_999_999_999_999)

# Collection equality — Array#include? and Hash key matching must
# canonicalise BigInt by value (two independently-allocated 2^64
# BigInts compare equal as members / keys). Before the ruby_eq
# fix, both returned false / nil.
a = [9_999_999_999_999_999_999, 1, 2]
puts a.include?(9_999_999_999_999_999_999)
h = {}
h[9_999_999_999_999_999_999] = "yes"
puts h[9_999_999_999_999_999_999]

# Mixed Int / BigInt equality through ruby_eq — the Array#include?
# path must canonicalise across types (BigInt that fits in i64
# already demotes via bigint_to_value, but a value that genuinely
# overflows must still compare equal to a literal of the same
# value passed as an Int operand). 2^63 is the smallest such case.
puts [9_223_372_036_854_775_808].include?(9_223_372_036_854_775_808)
