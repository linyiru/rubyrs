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

# Path B surrogates — three flavours, increasing realism:
#
#   `b_int`  — Integer with bare op (`.to_i`, `n + 1`). FLOOR
#              of what a Rust primitive could achieve; `+` here
#              hits BinOpInt's fused fast path which a real
#              `Value::Time + Int` could NOT use (BinOpInt is
#              hardcoded to (Int, Int) operand pair). v1 of this
#              bench reported only this row; it overstated the
#              A/B gap.
#
#   `b_send` — `n.send(:+, 1)` style, forcing `do_call`'s
#              method-dispatch path so the call routes through
#              `primitive_call` instead of `BinOpInt`. EXACTLY
#              the same dispatch shape a Rust `Value::Time`
#              primitive arm would carry. The realistic Path B
#              row.
#
#   `b_range` — `(sec..sec+1)` carries (begin, end) the same
#              way a hypothetical `HeapObj::Time { sec, nsec }`
#              would. `Range#begin` is a `primitive_call` arm
#              against a heap-backed value — directly comparable
#              to `time.to_i` on a Rust-side Time. Used as the
#              SECOND realistic Path B floor for the to_i shape.
t_b = 1_700_000_000
t_b_other = 1_700_000_001
t_b_range = (1_700_000_000..1_700_000_001)

scenario = ENV["BENCH_SCENARIO"] || "a_to_i"

case scenario
when "a_to_i"
  # Path A: user-class method call N times. Measures the frame
  # push/pop + LoadIvar + Return cost.
  N.times { t_a.to_i }
when "b_to_i"
  # Path B floor: bare `Integer#to_i` primitive_call arm.
  N.times { t_b.to_i }
when "b_to_i_send"
  # Path B realistic: same primitive_call arm but routed through
  # `send` so the dispatch site doesn't get a chance to inline.
  # Closest to what a Rust `(Value::Time, "to_i", [])` arm
  # would actually pay.
  N.times { t_b.send(:to_i) }
when "b_to_i_range"
  # Path B realistic alt: `Range#begin` against a heap-backed
  # value carrying two inner fields, mirroring how a
  # `HeapObj::Time { sec, nsec }` would shape access to `sec`.
  N.times { t_b_range.begin }
when "a_plus"
  # Path A: user-class `+` invocation N times. Adds an Object +
  # ivar Hash allocation per iteration on top of the dispatch
  # cost (this is the alloc-pressure axis of the comparison).
  N.times { t_a + 1 }
when "b_plus"
  # Path B floor: Integer + 1 via BinOpInt fused fast path.
  # NOT a realistic Path B Time arithmetic measurement — a real
  # `Value::Time + Int` couldn't use this path because BinOpInt
  # is hardcoded to (Int, Int) operands. Kept as the LOWER
  # bound only.
  N.times { t_b + 1 }
when "b_plus_send"
  # Path B realistic: `n.send(:+, 1)` forces the dispatch through
  # `do_call`'s primitive_call path (no BinOpInt fusion). This
  # is the SHAPE a real `Value::Time + Int` arm would pay — same
  # method dispatch + primitive arm match, just no per-op heap
  # alloc (Int + Int returns an Int, not a new heap slot, where
  # a real Time + Int would allocate a fresh Time). The actual
  # cost would be `b_plus_send + heap_alloc_overhead`.
  N.times { t_b.send(:+, 1) }
when "a_cmp"
  # Path A: <=> via user method. Method call + ivar load + Int
  # cmp.
  N.times { t_a <=> t_a_other }
when "b_cmp"
  # Path B floor: Integer <=> Integer (primitive arm).
  N.times { t_b <=> t_b_other }
when "b_cmp_send"
  # Path B realistic: same arm via send.
  N.times { t_b.send(:<=>, t_b_other) }
when "a_construct"
  # Path A construction cost: Object alloc + ivar HashMap +
  # initialize method dispatch per iteration.
  N.times { TimeA.new(t_b, 0) }
when "b_construct_range"
  # Path B realistic construct: `(sec..sec+1)` allocates a
  # `HeapObj::Range` with two inner Value slots — same shape a
  # `HeapObj::Time { sec, nsec }` would carry. The closest
  # primitive-shaped allocation available without changing the
  # interpreter.
  N.times { (t_b..t_b_other) }
when "b_construct"
  # Path B baseline: Integer doesn't have a construction concept
  # the way Time does, so this measures a bare Array.new(2) as a
  # rough proxy for "alloc a small heap object with 2 fields".
  # Lower bound only.
  N.times { Array.new(2, 0) }
when "a_workload"
  # Realistic Time-shaped workload mix — what one iteration of
  # a Sinatra-style request handler or a per-log-line generator
  # would actually do: construct a Time, read its seconds,
  # compute a delta against a reference, compare.
  ref = TimeA.new(1_699_999_000, 0)
  N.times do |i|
    t = TimeA.new(1_700_000_000 + i, 0)
    _delta = t.to_i - ref.to_i
    _is_after = (t <=> ref) > 0
  end
when "b_workload"
  # Path B realistic-workload equivalent. Uses send-dispatched
  # primitive ops + Range construction so the comparison stays
  # apples-to-apples with the per-op rows above. Each iteration:
  #   - `(sec..sec+1)` allocates a heap-backed value (mirror of
  #     a HeapObj::Time alloc)
  #   - `.begin` reads the inner field (mirror of `time.to_i`)
  #   - `send(:-, ...)` + `send(:>, ...)` exercise the
  #     primitive_call dispatch path
  ref_b = (1_699_999_000..1_699_999_001)
  N.times do |i|
    t = (1_700_000_000 + i..1_700_000_001 + i)
    _delta = t.begin.send(:-, ref_b.begin)
    _is_after = t.begin.send(:>, ref_b.begin)
  end
else
  raise ArgumentError, "unknown BENCH_SCENARIO: #{scenario.inspect}. " \
    "Pick one of: a_to_i, b_to_i, b_to_i_send, b_to_i_range, " \
    "a_plus, b_plus, b_plus_send, a_cmp, b_cmp, b_cmp_send, " \
    "a_construct, b_construct, b_construct_range, " \
    "a_workload, b_workload"
end
