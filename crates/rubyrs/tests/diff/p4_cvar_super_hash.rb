# --- 1. hierarchy-shared cvar through hot (cached) sites
class Base
  @@v = 0
  def bump; @@v = @@v + 1; end
  def readv; @@v; end
end
class Sub < Base
  def subv; @@v; end
  def subw; @@v = @@v + 10; end
end
s = Sub.new; b = Base.new
50.times { s.bump }
50.times { b.bump }
s.subw
p b.readv, s.subv

# --- 2. ||= write-through on the settled owner (i18n Config shape)
class Cfg
  def be; @@be ||= "simple"; end
  def be=(x); @@be = x; end
  def avail; @@avail ||= nil; end   # stays nil -> re-stores every call
end
c = Cfg.new
40.times { c.be; c.avail }
c.be = "chain"
p c.be, c.avail.inspect

# --- 3. negative verdict then late creation on the SAME class (gen bump)
class Late
  def probe; defined?(@@late) ? @@late : :unset; end
end
l = Late.new
20.times { l.probe }
class Late; @@late = :created; end
p l.probe

# --- 4. cvar from a module method reached via extend (lexical cref)
module Basey
  def nk; @@nk ||= []; end
end
module Hosty; extend Basey; end
Hosty.nk << 1
Hosty.nk << 2
p Hosty.nk

# --- 5. class_variable_set/get reflection agrees with the op path
class Refl; @@r = 1; def r; @@r; end; end
Refl.class_variable_set(:@@r, 99)
p Refl.new.r
Refl.class_variable_set(:@@fresh, 7)
p Refl.class_variable_get(:@@fresh)
p Refl.class_variable_defined?(:@@r), Refl.class_variable_defined?(:@@nope)

# (toplevel `@@x` is a documented rubyrs leniency — CRuby raises
# "class variable access from toplevel" — so it stays out of this
# CRuby-diffed fixture.)

# --- 7. super through hot cached sites + mid-stream redefinition
class PP
  def [](k); "P:#{k}"; end
end
class CC < PP
  def [](k); "C(#{super})"; end
end
cc = CC.new
acc = nil
60.times { |i| acc = cc[i] }
p acc
class PP
  def [](k); "P2:#{k}"; end
end
p cc[1]

# --- 8. super under define_method with two runtime names (one op site)
class DM
  def alpha; "A"; end
  def beta; "B"; end
end
class DMS < DM
  [:alpha, :beta].each { |m| define_method(m) { "#{m}(" + super() + ")" } }
end
d = DMS.new
p d.alpha, d.beta, d.alpha, d.beta

# --- 9. super with sibling receivers + a module inserted mid-stream
class Shape; def area; 1; end; end
class Sq < Shape; def area; super * 2; end; end
class Ci < Shape; def area; super * 3; end; end
shapes = [Sq.new, Ci.new, Sq.new, Ci.new]
p shapes.map { |x| x.area }
module Doubler; def area; super * 10; end; end
class Sq; prepend Doubler; end
p shapes.map { |x| x.area }

# --- 10. splat/kw super shapes (ApplySuper family carries a cid too)
class VarP
  def go(*a, **kw); "P(#{a.inspect},#{kw.inspect})"; end
end
class VarC < VarP
  def go(*a, **kw); "C[" + super + "]"; end
end
v = VarC.new
20.times { v.go(1, x: 2) }
p v.go(1, x: 2)

# --- 11. Hash merge/slice/except buckets: canonical semantics
h = { a: 1, b: 2, c: 3 }
30.times { h.merge({ d: 4 }); h.slice(:a, :c); h.except(:b) }
p h.merge({ d: 4 })
p h.merge(d: 4, a: 9)
p h.merge({ d: 4 }, { e: 5 })
p h.merge
p h.slice(:c, :a), h.slice, h.slice(:zz)
p h.except(:b, :zz), h.except
keys = [:a, :c]
p h.slice(*keys), h.except(*keys)

# --- 12. merge coercion + TypeError shape through the bucket
class Cfgish; def to_hash; { z: 26 }; end; end
p h.merge(Cfgish.new)
begin; h.merge(42); rescue TypeError => e; p e.message; end

# --- 13. default-proc travels on merge; frozen receiver stays fine
hd = Hash.new { |x, k| x[k] = "d#{k}" }
m = hd.merge({ x: 1 })
p m[:unseen], m[:x]
fh = { q: 1 }.freeze
p fh.merge(w: 2), fh.slice(:q), fh.except(:q)

# --- 14. user hash/eql? keys stay canonical through the bucket
class OddKey
  attr_reader :n
  def initialize(n); @n = n; end
  def hash; 7; end
  def eql?(o); o.is_a?(OddKey) && o.n == @n; end
end
hk = { OddKey.new(1) => "one", OddKey.new(2) => "two" }
p hk.slice(OddKey.new(2)).values
p hk.except(OddKey.new(1)).values
p hk.merge({ OddKey.new(1) => "uno" }).values.sort

# --- 15. subclass override precedence (tagged receiver declines)
class MyH < Hash
  def merge(*o); "custom"; end
end
mh = MyH.new; mh[:k] = 1
p mh.merge({ a: 1 })
p mh.slice(:k)   # no override -> Hash#slice via the canonical subclass path

# --- 16. reopen turns the buckets off (override wins after warm-up)
class Hash
  def except(*keys); "reopened"; end
end
p h.except(:a)
p h.merge({ d: 4 })  # merge/slice still canonical (lumped flag off is perf-only)
