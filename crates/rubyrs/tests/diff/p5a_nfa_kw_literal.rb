# Campaign P5a: NfaPlan kw-literal-defaults serve — a non-fixed-arity
# method whose EVERY kwarg is optional-with-literal-default binds
# stack-direct on bare Call sites (zero kwargs, mask 0), while every
# kwargs-carrying route declines to the general binder. Plus the
# fresh-per-call literal contract (CRuby re-evaluates the default
# expression each call, so a mutated Str default must NOT leak).

# --- 1. the AM `translations(do_init: false)` shape: hot bare calls
class T
  def translations(do_init: false)
    [@n = (@n || 0) + 1, do_init]
  end
end
t = T.new
acc = nil
60.times { acc = t.translations }
p acc
p t.translations(do_init: true)   # CallKw -> general binder
p t.translations                  # back to the serve

# --- 2. every literal kind, zero-kwargs bare call vs kwargs call
class K
  def kinds(i: 42, f: 1.5, s: "str", y: :sym, tt: true, ff: false, n: nil)
    [i, f, s, y, tt, ff, n]
  end
end
k = K.new
30.times { k.kinds }
p k.kinds
p k.kinds(f: 2.5, n: :given)
p k.kinds(**{ i: 0 })
p k.kinds(**{})                   # empty kwsplat -> zero kwargs (serve)

# --- 3. Str default mutation: each call gets a FRESH string
class M
  def mut(s: "x")
    s << "y"
    s
  end
end
m = M.new
p m.mut, m.mut, m.mut             # "xy" every time, never "xyy"
p m.mut(s: "a".dup)               # caller-supplied, untouched path

# --- 4. positional shapes around the kw region
class Sh
  def opt(a, b = :d, k: 1) = [a, b, k]
  def rest(a, *r, k: 2) = [a, r, k]
  def post(a, *r, c, k: 3) = [a, r, c, k]
  def blk(a, k: 4, &b) = [a, k, b.nil?]
end
sh = Sh.new
40.times { sh.opt(1); sh.rest(1, 2, 3); sh.post(1, 2, 3); sh.blk(1) }
p sh.opt(1), sh.opt(1, 2), sh.opt(1, k: 9)
p sh.rest(1), sh.rest(1, 2, 3), sh.rest(1, k: 9)
p sh.post(1, 9), sh.post(1, 2, 3, 9), sh.post(1, 9, k: 9)
p sh.blk(1), sh.blk(1, k: 9)
p(sh.blk(1) { :blk })             # block form -> block paths, &b bound

# --- 5. splat identity: the rest Array is fresh per call
r1 = sh.rest(1, 2)
r2 = sh.rest(1, 2)
r1[1] << :leak
p r2[1]

# --- 6. trailing brace-hash stays POSITIONAL (Ruby 3): arity error
class P3
  def kw_only(k: 1) = k
  def one_pos(h, k: 1) = [h, k]
end
p3 = P3.new
20.times { p3.kw_only; p3.one_pos({ a: 1 }) }
p p3.one_pos({ a: 1 })
begin
  p3.kw_only({ a: 1 })
rescue ArgumentError => e
  p e.message
end

# --- 7. ineligible kw shapes keep the general binder's answers
class Inel
  def req(a:) = a
  def computed(a: [1]) = a
  def mixed(a: 1, b: [2]) = [a, b]
  def kwrest(a: 1, **rest) = [a, rest]
end
inel = Inel.new
20.times { inel.computed; inel.mixed; inel.kwrest }
begin
  inel.req
rescue ArgumentError => e
  p e.message
end
p inel.req(a: 5)
c1 = inel.computed
c1 << :own
p inel.computed                   # computed default: fresh [1] per call
p inel.mixed, inel.kwrest, inel.kwrest(b: 2)

# --- 8. send re-aim: zero kwargs serves, kwargs decline
p t.send(:translations)
p t.send(:translations, do_init: true)

# --- 9. redefinition swaps the plan with the method
class T
  def translations(do_init: true)
    [:v2, do_init]
  end
end
p t.translations
p t.translations(do_init: false)

# --- 10. implicit-self serve reaches private kw-lit methods
class Priv
  def call_it = secret
  private
  def secret(tag: :hidden) = tag
end
pv = Priv.new
30.times { pv.call_it }
p pv.call_it
begin
  pv.secret
rescue NoMethodError => e
  p e.class
end

# --- 11. super into a kw-lit method (leaves the bare-Call flag off)
class SupA
  def val(k: :a) = [:base, k]
end
class SupB < SupA
  def val(k: :b) = [:sub, k, super()]
end
sb = SupB.new
20.times { sb.val }
p sb.val, sb.val(k: :x)

# --- 12. body creates a block capturing the kw param (Shared-cell
# locals path: the frame isn't arena-eligible)
class Cap
  def collect(n, sep: "-")
    out = []
    n.times { |i| out << "#{i}#{sep}" }
    out.join
  end
end
cap = Cap.new
25.times { cap.collect(2) }
p cap.collect(3), cap.collect(2, sep: "+")

# --- 13. define_method closure with kw default keeps the closure path
# (kwargs-PASSING calls to a kw-block installed as a method are a
# pre-existing documented gap — see Proto::block_kw_params — so only
# the zero-kwargs default path is diffed here.)
class DM
  define_method(:dm) { |a, k: :dm| [a, k] }
end
dm = DM.new
20.times { dm.dm(1) }
p dm.dm(1)
