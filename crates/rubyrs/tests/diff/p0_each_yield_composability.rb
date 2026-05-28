# P0 (ADR 0023): pin the Tier-1 prerequisite that A3β's
# `each_fiber_path` depends on — user-defined `def each;
# yield ...; end` must compose with externally-supplied
# blocks in exactly CRuby's shape.
#
# Existing diff_cruby fixtures cover related but distinct
# shapes (yield_through_nested_block — yield from inside a
# block frame; enumerable_stub — Enumerable include
# survives + delegating each). This fixture pins the
# specific shape A3β will hit: a method whose body is
# directly `yield val_1; yield val_2; ...` (no Array
# delegation), invoked with an external block that
# observes every yield in order.
#
# If this fixture starts failing, A3β's `each_fiber_path`
# would mis-route any body class that uses this idiom —
# which is most hand-rolled Rack 3 enumerable bodies.

# --- Basic: direct multi-yield in def each ---
class TwoChunkBody
  def each
    yield "chunk-a"
    yield "chunk-b"
  end
end
out = []
TwoChunkBody.new.each { |v| out << v }
puts out.inspect                               # ["chunk-a", "chunk-b"]

# --- Yield count matches caller's iteration count ---
# Probes that no extra block invocation happens after the
# final yield (defensive against a buggy implementation
# that re-pumps the block on method return).
class CountingBody
  def each
    yield 1
    yield 2
    yield 3
  end
end
n = 0
CountingBody.new.each { |_| n += 1 }
puts n                                          # 3

# --- Block raises mid-iteration → propagates out cleanly ---
# The Fiber path in A3β catches raises at the poll_frame
# boundary; that requires the underlying each → yield →
# block to surface exceptions normally (not swallow them
# into the method's return value).
class RaisesAt
  def initialize(boom_at)
    @boom_at = boom_at
  end
  def each
    yield :first
    yield :second
    yield :third
  end
end
got = []
begin
  RaisesAt.new(:second).each do |sym|
    got << sym
    raise "deliberate" if sym == :second
  end
rescue RuntimeError => e
  got << "rescued: #{e.message}"
end
puts got.inspect                                # [:first, :second, "rescued: deliberate"]

# --- next in block skips to next yield ---
# Rack 3 streaming handlers can use `next` to skip an
# empty chunk; verify the each loop continues past it.
class ThreeYields
  def each
    yield 1
    yield 2
    yield 3
  end
end
seen = []
ThreeYields.new.each do |v|
  next if v == 2
  seen << v
end
puts seen.inspect                               # [1, 3]

# --- break in block exits each early ---
# Common in long-poll handlers: stream a few events,
# then break on a condition. Verify each returns
# normally without raising.
class ManyYields
  def each
    10.times { |i| yield i }
  end
end
first_two = []
result = ManyYields.new.each do |v|
  first_two << v
  break :early if v == 1
end
puts first_two.inspect                          # [0, 1]
puts result.inspect                             # :early
