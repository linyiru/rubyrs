# The send-family fast buckets (vm/dispatch.rs) gate on IC-backed
# lookup misses that are revalidated via method_gen. This fixture
# defines overrides AFTER the call sites are warm (looped hot), so a
# stale-IC bug would keep serving the builtin shape after the
# override lands. Three-way gate: run under default, RUBYRS_JIT_TIER2
# and STRESS_GC.

class Warmed
  def pub; :pub; end
  def probe(sym); respond_to?(sym); end
end

w = Warmed.new

# --- respond_to? override defined after warm ---
acc = 0
200.times { acc += 1 if w.respond_to?(:pub) }
p acc
class Warmed
  def respond_to?(name, include_all = false)
    "custom:#{name}:#{include_all}"
  end
end
p w.respond_to?(:pub)          # the override must win post-warm
p w.respond_to?(:pub, true)

# --- respond_to_missing? defined after warm ---
class Lazy
  def real; :real; end
end
l = Lazy.new
warm_hits = 0
200.times { warm_hits += 1 if l.respond_to?(:ghost) }
p warm_hits                     # 0 — no hook yet
class Lazy
  def respond_to_missing?(name, include_all = false)
    name == :ghost
  end
end
p l.respond_to?(:ghost)         # true via the hook, post-warm
p l.respond_to?(:other)
p l.respond_to?(:real)

# --- public_send / send re-aim with overrides landing after warm ---
class Target
  def hot(x); x + 1; end
end
t = Target.new
sum = 0
200.times { |i| sum += t.public_send(:hot, i) }
p sum
200.times { |i| sum += t.send(:hot, i) }
p sum

# a subclass override of public_send itself, after warm
class SubTarget < Target; end
st = SubTarget.new
200.times { |i| st.public_send(:hot, i) }
class SubTarget
  def public_send(name, *args)
    "hijacked:#{name}:#{args.inspect}"
  end
end
p st.public_send(:hot, 1)       # the user override must win
p t.public_send(:hot, 1)        # the base class is unaffected

# a `send` override after warm (reserved-name rule: __send__ ignores it)
class SendOver
  def go(x); x * 2; end
end
so = SendOver.new
200.times { |i| so.send(:go, i) }
class SendOver
  def send(name, *args)
    "sent:#{name}"
  end
end
p so.send(:go, 3)
p so.__send__(:go, 3)           # __send__ is reserved — real dispatch

# --- target method redefined after warm (dynamic-name resolution) ---
class Mutant
  def m; :old; end
end
mu = Mutant.new
200.times { mu.public_send(:m) }
p mu.public_send(:m)
class Mutant
  def m; :new; end
end
p mu.public_send(:m)

# --- visibility flipped after warm ---
class Flip
  def f; :f; end
end
fl = Flip.new
200.times { fl.public_send(:f) }
class Flip
  private :f
end
begin
  fl.public_send(:f)
rescue NoMethodError => e
  puts "post-warm-flip: #{e.message}"
end
p fl.send(:f)

# --- bare (implicit-self) send warm loop ---
class BareSend
  def helper(x); x - 1; end
  def drive
    total = 0
    200.times { |i| total += send(:helper, i) }
    total
  end
end
p BareSend.new.drive

# --- respond_to? include_all truthiness through the bucket ---
class VisMix
  def pub2; :p2; end
  protected def prot2; :x; end
  private def priv2; :y; end
end
v = VisMix.new
p 100.times.map { v.respond_to?(:prot2) }.uniq
p 100.times.map { v.respond_to?(:prot2, true) }.uniq
p 100.times.map { v.respond_to?(:priv2, :truthy) }.uniq
p 100.times.map { v.respond_to?(:priv2, nil) }.uniq
