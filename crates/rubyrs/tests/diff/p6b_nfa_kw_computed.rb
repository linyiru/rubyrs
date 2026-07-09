# Campaign P6b, Item 2: NfaPlan COMPUTED-default kwargs serve. A
# non-fixed-arity method whose every kwarg is OPTIONAL (literal OR
# computed default; no required kwarg, no **kwrest) now binds
# stack-direct on bare-Call sites passing zero kwargs — the serve
# leaves each computed slot Nil and stamps kw_given_mask = 0, so the
# body prologue (Op::JumpIfKwArgGiven) evaluates every computed default
# fresh, exactly the general binder's zero-kwargs outcome. Every
# kwargs-carrying route still declines to the general binder. Required
# kwargs and **kwrest stay INELIGIBLE (they keep the missing-keyword
# ArgumentError / peel on the binder).

DIG = 40   # constant-ref computed default (the AM Float::DIG shape)

# --- 1. the AM `validate_each(record, attr, value, precision:
# Float::DIG, scale: nil)` shape: 3 required positionals, one computed
# (constant) kwarg + one literal (nil) kwarg, called BARE and hot.
class V
  def validate_each(record, attr, value, precision: DIG, scale: nil)
    [record, attr, value, precision, scale]
  end
end
v = V.new
acc = nil
200.times { acc = v.validate_each(:rec, :age, 30) }
p acc                               # [:rec, :age, 30, 40, nil]
p v.validate_each(:r, :a, 1, precision: 7)      # CallKw -> general binder
p v.validate_each(:r, :a, 1, scale: 2)          # CallKw
p v.validate_each(:r, :a, 1)                    # back to the serve

# --- 2. computed default referencing an EARLIER POSITIONAL param.
class C
  def add(a, b: a + 1) = [a, b]
end
c = C.new
200.times { c.add(10) }
p c.add(10), c.add(10, b: 99)       # [10, 11], [10, 99]

# --- 3. computed default referencing an EARLIER KWARG (literal then
# computed): the prologue runs after the literal slot is filled.
class C2
  def chain(a: 1, b: a + 1) = [a, b]
end
c2 = C2.new
200.times { c2.chain }
p c2.chain, c2.chain(a: 5), c2.chain(a: 5, b: 100)  # [1,2] [5,6] [5,100]

# --- 4. computed default is evaluated FRESH per call and ONLY when the
# kwarg is absent (a side-effecting default).
$calls = 0
class Ctr
  def bump; $calls += 1; $calls; end
  def take(x: bump) = x
end
ct = Ctr.new
$calls = 0
r = nil
50.times { r = ct.take }            # each call re-evaluates bump
p r                                 # 50
p ct.take(x: 999)                   # given -> default NOT evaluated
p $calls                            # 50 (bump not called for the given case)

# --- 5. mutable computed default is FRESH per call (no cross-call leak).
class Mut
  def arr(a: []) = a
end
m = Mut.new
50.times { m.arr }
r1 = m.arr; r1 << :leak
p m.arr                             # [] — fresh, no leak

# --- 6. computed kwarg alongside positional OPTIONALS and *rest.
class Sh
  def opt(a, b = :d, k: a) = [a, b, k]
  def rst(a, *r, k: r.length) = [a, r, k]
end
sh = Sh.new
100.times { sh.opt(1); sh.rst(1, 2, 3) }
p sh.opt(1), sh.opt(1, 2), sh.opt(1, 2, k: 9)
p sh.rst(1), sh.rst(1, 2, 3), sh.rst(1, 2, k: 9)

# --- 7. splat identity: the rest Array is still fresh per call even
# with a computed kwarg reading it.
r1 = sh.rst(1, 9); r2 = sh.rst(1, 9)
r1[1] << :x
p r2[1]                             # [9]

# --- 8. implicit-self (CallNoRecv) serve — the validate_each dispatch
# shape (a self method calling another self method with zero kwargs).
class Runner
  def run = compute(:x, :y, 3)
  def compute(a, b, c, precision: DIG, scale: nil) = [a, b, c, precision, scale]
end
rn = Runner.new
200.times { rn.run }
p rn.run                            # [:x, :y, 3, 40, nil]

# --- 9. send re-aim: zero-kwargs serves, kwargs decline (both correct).
p v.send(:validate_each, :r, :a, 2)                 # [:r, :a, 2, 40, nil]
p v.send(:validate_each, :r, :a, 2, precision: 8)   # [:r, :a, 2, 8, nil]

# --- 10. super into a computed-default method (super leaves the
# bare-Call flag off -> general binder, computed default still fresh).
class Base
  def calc(a, k: a * 2) = [:base, a, k]
end
class Sub < Base
  def calc(a, k: a * 3) = [:sub, super(a)]
end
sub = Sub.new
50.times { sub.calc(4) }
p sub.calc(4)                       # [:sub, [:base, 4, 8]]

# --- 11. REQUIRED kwarg stays INELIGIBLE (missing-keyword error path
# stays on the general binder).
class Req
  def need(a:) = a
  def mix_req(a, b: a + 1, c:) = [a, b, c]
end
rq = Req.new
20.times { rq.need(a: 1) }
begin; rq.need; rescue ArgumentError => e; p e.message; end
p rq.need(a: 5)
p rq.mix_req(1, c: 9)               # [1, 2, 9]
begin; rq.mix_req(1); rescue ArgumentError => e; p e.message; end

# --- 12. **kwrest present -> INELIGIBLE (peel stays on the binder).
class KR
  def kr(a: 1, **rest) = [a, rest]
end
kr = KR.new
20.times { kr.kr }
p kr.kr, kr.kr(a: 2, z: 9)          # [1, {}], [2, {z: 9}]

# --- 13. redefinition swaps the plan with the method (proto_idx keyed).
class C
  def add(a, b: a * 10) = [:v2, a, b]
end
p c.add(3)                          # [:v2, 3, 30]
p c.add(3, b: 7)                    # [:v2, 3, 7]

# --- 14. brace-hash trailing arg stays POSITIONAL (Ruby 3): a computed
# kwarg method called `f({k: v})` binds the hash to a positional, which
# for a kwarg-only-tail method is an arity error.
class P3
  def kwonly(k: 1 + 1) = k
end
p3 = P3.new
20.times { p3.kwonly }
p p3.kwonly                         # 2
begin
  p3.kwonly({ a: 1 })
rescue ArgumentError => e
  p e.message
end
