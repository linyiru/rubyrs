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

# Inverse-receiver operator method-call — Int receiver with
# BigInt arg (`1.+(big)`, `1.send(:+, big)`). Pre-cycle-4 the
# bigint_primitive hook only fired when receiver was BigInt, so
# this fell through to Int#+ which couldn't handle BigInt args.
puts 1.+(9_223_372_036_854_775_808)
puts 1.send(:+, 9_223_372_036_854_775_808)

# Range#inject / Array#inject must keep folding once the
# accumulator promotes to BigInt — pre-cycle-4 they hit the
# wildcard arm and bailed the whole primitive.
puts (1..30).inject(:*)
puts [10_000_000_000, 10_000_000_000, 10_000_000_000].inject(:+)

# Array#sum / Range#sum overflow promotion — pre-cycle-5 these
# used wrapping_add and silently lost precision the moment the
# running total crossed i64. Range#sum uses the n*(a+b)/2 closed
# form which can overflow at the multiplication step alone.
big = 4_611_686_018_427_387_904  # 2^62
puts [big, big, big].sum
puts (1..10_000_000_000).sum
puts [1, 2, 3, big].sum

# `<=>` across Int / BigInt — pre-cycle-8 the Int catch-all in
# primitive.rs (`(Value::Int, "<=>", [_]) => Nil`) shadowed
# bigint_primitive's downstream handling and returned nil for
# `1 <=> big`. respond_to?(:<=>) reports true universally, so
# the catch-all returning nil silently violated the contract
# Comparable relies on.
huge = 9_999_999_999_999_999_999
puts huge <=> 1
puts 1 <=> huge
puts huge <=> huge

# Array#sum with BigInt elements (not just BigInt accumulator).
# Pre-cycle-9 the wildcard arm `_ => return Ok(None)` bailed the
# whole primitive when ANY element was BigInt, even though
# `[bignum_literal].sum` is the canonical case.
puts [9_223_372_036_854_775_808].sum
puts [10, 9_223_372_036_854_775_808, 100].sum

# Range#sum where the closed-form product `n * (bi + end_inc)`
# overflows i64 even though `end_inc - bi` fits. n = 4e9,
# bi + end_inc ≈ 4e9, n*sum ≈ 1.6e19 > i64::MAX (≈9.2e18). The
# fast path must detect this via checked_mul and fall through
# to the BigInt branch; pre-cycle-9 the wrap was silent.
puts (1..4_000_000_000).sum

# Singleton inclusive range — `(N..N).inject(:op)`. Pre-cycle-12
# this hung the host because the loop was unconditional and
# i = bi + 1 > end_inc never satisfied the `i == end_inc` break.
puts (1..1).inject(:+)
puts (5..5).inject(:*)

# Int#+/-/* operator method-call with i64-overflow result. Pre-
# cycle-12 the method-call form (`a.+(b)`, `a.send(:+, b)`)
# went through numeric_call's plain `+` which wraps; only the
# Op::BinOp expression form (`a + b`) promoted. Now both
# match Op::BinOp's behaviour via a pre-primitive_call intercept.
puts 9_223_372_036_854_775_807.+(1)
puts 9_223_372_036_854_775_807.send(:+, 1)
puts 1_000_000_000.send(:*, 1_000_000_000).send(:*, 1_000_000_000)

# === /code-review post-merge findings (commit batch on top of cycle 13) ===

# Array#min/#max/#sort across Int/BigInt — pre-fix used
# value_cmp_v which had no BigInt arm and returned None, surfacing
# as NoMethodError. Now goes through value_cmp_v_heap.
puts [9_223_372_036_854_775_808, 5].min
puts [5, 9_223_372_036_854_775_808].max
puts [9_223_372_036_854_775_808, 5, 100].sort.inspect

# Range#include? / #cover? with BigInt arg on Int-bounded range —
# any reachable BigInt at this point is genuinely outside i64
# range, so the answer is always false (BigInt that would have
# fit i64 was demoted via bigint_to_value).
puts (1..10).include?(9_223_372_036_854_775_808)
puts (1..10).cover?(9_223_372_036_854_775_808)

# BigInt Phase A predicates — pure read-only methods that don't
# need heap mutation but were missing from the cycle-13 surface.
big = 9_999_999_999_999_999_999
puts big.zero?
puts big.positive?
puts(-1_000_000_000_000_000_000_000_000.positive?)  # negative BigInt (literal-promoted)
puts(-1_000_000_000_000_000_000_000_000.negative?)
puts big.even?
puts big.odd?
puts big.to_i
puts big.respond_to?(:zero?)
puts big.respond_to?(:to_i)

# Block-form send on BigInt — pre-fix raised NoMethodError because
# do_call_block lacked the bigint_primitive hook do_call had.
puts big.send(:to_s, &proc{|x| x})
puts 1.send(:+, 9_223_372_036_854_775_808, &proc{|x| x})

# `<=>` cross-direction (Int recv with BigInt rhs). The Int <=>
# catch-all in primitive.rs has a carve-out for BigInt rhs so
# bigint_primitive's <=> branch can fire. Pin that ordering with
# a fixture (was only covered for big <=> small before).
puts(1 <=> 9_999_999_999_999_999_999)
puts(9_999_999_999_999_999_999 <=> 1)
puts(9_999_999_999_999_999_999 <=> 9_999_999_999_999_999_999)

# sprintf %d / %+d on BigInt. Pre-fix raised TypeError with the
# self-contradictory "no implicit conversion of Integer to Integer".
puts '%d' % big
puts '%+d' % big
puts ('|%20d|' % big)
