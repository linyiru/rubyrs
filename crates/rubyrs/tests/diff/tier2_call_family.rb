# Tier-2 wave-2 IC-fast call battery (ADR 0037): explicit-recv / self-recv /
# LoadLocalCall sites inside warmed (tier-2-compiled) bodies must behave
# byte-identically to the interpreter across every "surprise" the dedicated
# t2_call helper must decline: visibility errors, method_missing, method
# redefinition after warm native->native, non-Object receivers, kwargs
# shapes, assignment-syntax calls, send-visibility-bypass, and
# rescue/backtrace through mixed native->native frames.
# Warm loops run well past the tier-2 adaptive compile threshold
# (base 1024 + 16/op) so under RUBYRS_JIT_TIER2=1 the hot bodies are
# native when the behavior checks run.

class Cell
  attr_reader :v
  attr_accessor :w
  def initialize(v) = (@v = v; @w = 0)
  def bump(a) = @v + a
  def pair(a, b) = a * 10 + b + @v
  def w2=(x)
    @w = x * 2
    :writer_return_ignored
  end

  def secret = :leaked
  private :secret
  def shielded = :also_leaked
  protected :shielded

  def probe_shield(o) = o.shielded     # protected: legal ONLY on same-class recv
  def own_secret = secret              # implicit-self private -> legal
end

class Prober
  # explicit-recv private/protected from an UNRELATED class -> NoMethodError
  def probe_secret(o) = o.secret
  def probe_shield(o) = o.shielded
end

class Chain
  def initialize(n) = @n = n
  def run(c)
    # explicit-recv 0/1/2-arg + LoadLocalCall fusion + self-recv chain
    local = c
    t = local.v
    t += c.bump(1) + c.pair(2, 3)
    t += helper(t)
    t
  end
  def helper(x) = x > 100 ? big(x) : small(x)
  def big(x) = x - @n
  def small(x) = x + @n
  def boom(depth)
    raise "boom-at-#{depth}" if depth.zero?
    boom(depth - 1)
  end
  def deep(k) = k.zero? ? 0 : 1 + deep(k - 1)
  def kwish(a, b: 7) = a + b
end

c = Cell.new(5)
ch = Chain.new(3)
acc = 0
4000.times { acc += ch.run(c) }
puts acc

# -- assignment-syntax call on a warmed writer: expression value is the RHS
r = 0
4000.times { |i| r = (c.w2 = i) }
puts r, c.w

# -- visibility through warmed explicit-recv sites
pr = Prober.new
begin
  pr.probe_secret(c)
rescue NoMethodError => e
  puts e.message
end
begin
  pr.probe_shield(c)
rescue NoMethodError => e
  puts e.class
end
puts c.probe_shield(Cell.new(1))
puts c.own_secret
puts c.send(:secret) # visibility bypass must still work around warm sites

# -- method_missing via a warmed site
class Ghost
  def method_missing(name, *a) = "mm:#{name}:#{a.inspect}"
  def respond_to_missing?(n, p = false) = true
end
def poke(g, n) = g.phantom(n)
g = Ghost.new
2000.times { |i| poke(g, i) }
puts poke(g, 41)

# -- redefinition AFTER warm native->native: the IC must re-resolve
class Cell
  def bump(a) = @v * 100 + a
end
puts ch.run(c)

# -- kwargs shape declines to the general binder
puts ch.kwish(1), ch.kwish(1, b: 2)

# -- deep native->native recursion (past the 96-level native nesting cap)
puts ch.deep(3000)

# -- rescue + backtrace through mixed native->native frames
begin
  ch.boom(40)
rescue RuntimeError => e
  puts e.message
  # Frame identity (file:line) is the contract; the in-'…' label format is a
  # known cosmetic divergence (CRuby 3.4 prints 'Chain#boom', rubyrs 'boom').
  puts e.backtrace.first(3).map { |l| l[%r{[^/]+:\d+}] }
end

# -- non-Object receivers at warmed sites keep the primitive fast paths
class Mixer
  def mix(s, a, h)
    s.size + a.size + h.size + a.include?(2).to_s.size + h.fetch(:k, 0)
  end
end
m = Mixer.new
s, a, h = "abcd", [1, 2, 3], { k: 9 }
t = 0
4000.times { t += m.mix(s, a, h) }
puts t
