# Tier-2 LITE t2_call battery (ADR 0037 wave-4 follow-on).
#
# Call-bearing FRAMELESS bodies: a Call/CallNoRecv/LoadLocalCall op inside a
# frame-lite activation resolves through the site IC and either serves the
# callee frameless (getter / zeroarg-native / rest-pred / fast-prim /
# lite->lite native chains) or MATERIALIZES the caller's frame — with
# outward-in cascading through nested lite activations — before the
# interpreter takes over. This battery pins the acid contract:
#
#   1. a raise inside an interpreted callee invoked from a lite caller shows
#      the caller's line (the materialize stamps ip at the call op);
#   2. `caller` from such a callee sees the (materialized) lite frames;
#   3. deep lite->lite recursion respects the native-depth cap and the
#      frame-cap discipline;
#   4. a mid-chain materialize pushes frames OUTWARD-IN (frame order on the
#      VM == the interpreter's — verified by backtrace order);
#   5. redefinition-after-warm of a lite->lite callee re-resolves;
#   6. ensure in a FRAMED caller runs on unwind through lite activations;
#   7. IC-cached bare-constant reads (LoadConstChain) serve frameless and
#      survive const-cache invalidation;
#   8. bare calls with a genuinely-nil / toplevel-main self keep do_call's
#      defining_class-gated routing (the deferred push carries the same
#      defining_class the framed push would).
#
# Backtrace lines are normalized to "file:line" (same normalization as
# tier2_framelite_battery).

LIMIT = 40

class Node
  def initialize(v)
    @v = v
    @label = nil
  end

  attr_reader :v

  # Lite leaf callees (each admits to frame-lite on its own).
  def double(x)
    x + x
  end

  def send_type?
    @v == :send
  end

  # Lite caller -> lite callee chains.
  def quad(x)
    double(double(x))
  end

  # Explicit-recv chain through another object.
  def sum_with(other, x)
    other.double(x) + @v
  end

  # LoadLocalCall fusion: local receiver, zero-arg callee.
  def probe(other)
    w = other
    w.send_type? ? 1 : 0
  end

  # Const-reading lite body (LoadConstChain inside a class scope).
  def capped
    @v > LIMIT ? LIMIT : @v
  end

  # Chain into an interpreted callee (never lite: string building) — the
  # caller materializes at the call op every time (breaker will settle it).
  def shout
    interp_name(@v)
  end

  def interp_name(v)
    "node-#{v}"
  end

  # Deep lite->lite self-recursion.
  def countdown(n)
    return 0 if n < 1
    countdown(n - 1) + 1
  end

  # Cascade shape: a -> b -> c where c hits a materialize edge MID-BODY
  # (the Str-valued ivar read declines the lite ivar-get, cascading the
  # whole live chain into real frames), then continues interpreted — with
  # an Int arg it completes, with a Str arg `@v + x` raises the canonical
  # TypeError whose backtrace must list c, b, a in interpreter order.
  def casc_a(x)
    casc_b(x)
  end

  def casc_b(x)
    casc_c(x)
  end

  def casc_c(x)
    t = @label
    t.nil? ? 0 : @v + x
  end

  # `caller` acid: lite caller -> interpreted callee that inspects caller.
  def who_called
    report_caller
  end

  def report_caller
    caller.first(2).map { |ln| ln[%r{[^/]+:\d+}] }
  end
end

nodes = (1..50).map { |i| Node.new(i) }

# ---- warm every shape past any compile threshold ----
acc = 0
nodes.each do |n|
  400.times do
    acc += n.quad(3)
    acc += n.sum_with(n, 2)
    acc += n.probe(n)
    acc += n.capped
    acc += n.countdown(5)
  end
end
puts acc

# 1. Raise inside an interpreted callee called from a (warm) lite caller:
#    the backtrace shows the lite caller's call line.
class Node
  def lite_wrap(x)
    boom_interp(x)
  end

  def boom_interp(x)
    raise ArgumentError, "no #{x}" if x > 3
    x
  end
end
n1 = Node.new(7)
2000.times { n1.lite_wrap(1) }
begin
  n1.lite_wrap(9)
rescue ArgumentError => e
  puts "E1 #{e.message}"
  e.backtrace.first(3).each { |ln| puts "  #{ln[%r{[^/]+:\d+}]}" }
end

# 2. `caller` from an interpreted callee under a lite caller.
2000.times { n1.who_called }
puts n1.who_called.inspect

# 3. Deep lite->lite recursion (past the native-depth cap; values exact).
puts n1.countdown(400)
puts nodes.map { |n| n.countdown(12) }.sum

# 4. Materialization cascade: a -> b -> c, c materializes mid-body (Str
#    ivar read) and continues interpreted; a Str arg then raises with the
#    frames in interpreter order. The raising call comes BEFORE the breaker
#    could settle the shape, so a live native chain is what cascades.
n1.instance_variable_set(:@label, +"L")
csum = 0
20.times { csum += n1.casc_a(4) }
begin
  n1.casc_a("x")
rescue TypeError => e
  puts "E4 #{e.class} csum=#{csum}"
  lines = e.backtrace.first(5).map { |ln| ln[%r{[^/]+:\d+}] }
  lines.chunk_while { |a, b| a == b }.map(&:first).first(4).each { |ln| puts "  #{ln}" }
end
# ... and past the breaker (chronic cascades settle to framed; values exact).
2000.times { csum += n1.casc_a(4) }
puts csum

# 5. Redefinition-after-warm of a lite->lite callee: the chain re-resolves.
#    (Symbol-returning shape: outside the jit-native compile families, so
#    this pins the TIER-2 IC re-resolution, not the known jit-native
#    baked-cross-call redefinition gap — see JIT_KNOWN_DIVERGENCES.)
class Node
  def tag_for(x)
    x > 2 ? :big : :small
  end

  def wrap_tag(x)
    tag_for(x)
  end
end
2000.times { n1.wrap_tag(5) }
puts n1.wrap_tag(5).inspect
class Node
  def tag_for(_x)
    :other
  end
end
puts n1.wrap_tag(5).inspect
puts nodes.first.wrap_tag(0).inspect

# 6. ensure in a FRAMED caller running on unwind through lite activations.
def with_ensure_marker(n)
  marker = :before
  n.casc_a("s")
  marker = :after
ensure
  puts "ensure marker=#{marker}"
end
begin
  with_ensure_marker(n1)
rescue TypeError => e
  puts "E6 #{e.class}"
end

# 7. Const-chain reads: warm serves, then invalidate the const caches (a
#    fresh const definition) — values stay exact across the refill.
puts nodes.map(&:capped).sum
LATE_CONST = 5
puts nodes.map(&:capped).sum + LATE_CONST

# 8. Toplevel-main lite chains (self is the toplevel main): bare-call
#    recursion at main keeps toplevel-method routing.
def top_leaf(x)
  x + 1
end

def top_wrap(x)
  top_leaf(x) + top_leaf(x)
end
t = 0
3000.times { t += top_wrap(1) }
puts t

# 8b. Genuinely-nil self: a NilClass method with a bare call must keep
#     resolving through NilClass (the defining_class-gated do_call arm),
#     never the toplevel table — warm or cold.
class NilClass
  def lite_flag
    42
  end

  def lite_probe
    lite_flag
  end
end
r = 0
3000.times { r += nil.lite_probe }
puts r

# 9. Interpreted-callee chronic shape (breaker settles; values exact).
outs = []
300.times { outs << n1.shout }
puts outs.uniq.inspect

# 10. Explicit-recv lite chain across objects + rest-pred-style callee mix.
puts nodes.map { |n| n.sum_with(nodes.first, 2) }.sum
puts nodes.map { |n| n.probe(nodes.last) }.sum
