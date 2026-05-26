# Micro-benchmark: Path A (pure-Ruby Time class via user-method
# dispatch) vs. Path B (Rust primitive, simulated by calling an
# equivalent method on a value whose dispatch already lives on the
# primitive_call fast path).
#
# Purpose: an empirical answer to "is Path A's dispatch overhead
# acceptable for the rubyrs embed niche, or do we need a Rust-side
# Time primitive?" — see the Time design discussion that led to
# this scaffold.
#
# How it works:
#
#   - Path A is emulated inline by defining a `TimeA` user class
#     with `@sec` / `@nsec` ivars and `to_i` / `+` / `<=>` methods.
#     This is the EXACT dispatch shape the vendored
#     `crates/rubyrs/src/preamble/time.rb` would have once Path A
#     lands — the only difference is where the .rb code lives.
#
#   - Path B is emulated by calling `Integer#to_i` (a primitive
#     arm in `vm/numeric.rs::numeric_call`). The dispatch shape
#     is identical to what a Rust-side `(Value::Time, "to_i", [])`
#     arm would carry — same `primitive_call` lookup table, same
#     branch overhead, same lack of frame push/pop. Integer is the
#     simplest Value variant on the fast path so it represents the
#     LOWER BOUND of what Path B could achieve.
#
# Loop count is `N`. Runner script (`perf/time_microbench.sh`)
# wraps each scenario in `/usr/bin/time` and reports min-of-5
# wall ms; the .rb itself just runs the scenario selected via
# the `BENCH_SCENARIO` env var (host-injected — see below).
#
# Output: nothing on stdout. Wall time is measured externally so
# the script body is as minimal as possible (no Time, no `p`).
# The runner reads `/usr/bin/time`'s wall-time line via stderr.

N = (ENV["BENCH_N"] || "1_000_000").gsub("_", "").to_i

# Path A surrogate — a user class that does what a pure-Ruby
# Time vendor would do for the three most-called shapes:
# `Time.now.to_i`, `t + offset`, `t <=> u`.
class TimeA
  def initialize(sec, nsec = 0)
    @sec = sec
    @nsec = nsec
  end

  def to_i
    @sec
  end

  def +(other)
    if other.is_a?(TimeA)
      # Time + Time is a TypeError in CRuby; keep the surrogate
      # honest by raising. We don't exercise this in the bench.
      raise TypeError, "no implicit conversion of TimeA into Integer"
    end
    TimeA.new(@sec + other.to_i, @nsec)
  end

  def <=>(other)
    return nil unless other.is_a?(TimeA)
    @sec <=> other.to_i
  end
end

# Pre-build a Time so the loop measures DISPATCH cost, not
# construction cost. Construction is its own measurement (see
# bench_construct below).
t_a = TimeA.new(1_700_000_000, 0)
t_a_other = TimeA.new(1_700_000_001, 0)

# Path B surrogate — Integer pre-computed for the same shape.
# Integer's `to_i` arm sits at `vm/numeric.rs:228`; the `+` op
# routes through the BinOp fast path (even cheaper than a method
# call, so a "real" Time primitive would still be slower than
# Integer+Integer because of the receiver-type check). For a
# fair comparison we use `.send(:+, other)` to force the slow-
# path BinOp dispatch — same routing a Rust-side Value::Time arm
# would carry.
t_b = 1_700_000_000
t_b_other = 1_700_000_001

scenario = ENV["BENCH_SCENARIO"] || "a_to_i"

case scenario
when "a_to_i"
  # Path A: user-class method call N times. Measures the frame
  # push/pop + LoadIvar + Return cost.
  N.times { t_a.to_i }
when "b_to_i"
  # Path B: primitive_call match arm N times. Measures the bare
  # dispatch + arm hit cost.
  N.times { t_b.to_i }
when "a_plus"
  # Path A: user-class `+` invocation N times. Adds an Object +
  # ivar Hash allocation per iteration on top of the dispatch
  # cost (this is the alloc-pressure axis of the comparison).
  N.times { t_a + 1 }
when "b_plus"
  # Path B equivalent — Integer + 1 via the BinOpInt fast path.
  # Doesn't exercise an allocation (Int + Int returns Int), so
  # this is the floor of "if Time were as fast as Integer".
  N.times { t_b + 1 }
when "a_cmp"
  # Path A: <=> via user method. Method call + ivar load + Int
  # cmp.
  N.times { t_a <=> t_a_other }
when "b_cmp"
  # Path B equivalent — Integer <=> Integer via primitive_call.
  N.times { t_b <=> t_b_other }
when "a_construct"
  # Path A construction cost: Object alloc + ivar HashMap +
  # initialize method dispatch per iteration.
  N.times { TimeA.new(t_b, 0) }
when "b_construct"
  # Path B baseline: Integer doesn't have a construction concept
  # the way Time does, so this measures a bare Array.new(2) as a
  # rough proxy for "alloc a small heap object with 2 fields".
  # Lower bound only.
  N.times { Array.new(2, 0) }
else
  raise ArgumentError, "unknown BENCH_SCENARIO: #{scenario.inspect}. " \
    "Pick one of: a_to_i, b_to_i, a_plus, b_plus, a_cmp, b_cmp, " \
    "a_construct, b_construct"
end
