# Binding-semantics battery for the non-fixed-arity (NFA) dispatch fast
# path (ADR 0031 increment 2): explicit-recv + implicit-self calls to
# user methods with optionals / *rest / post-required / &blk bind via a
# precomputed per-proto plan on IC hit. Every case below runs in a WARM
# loop (3x) so the plan-served path is exercised after the first call,
# and covers: default-expression scope/order/exactly-once, splat
# identity (fresh array per call), post-required (with and without
# rest), &blk-with-no-block, kwargs staying on the general path,
# private/protected visibility, ArgumentError shapes on warm sites,
# redefinition + subclass override after a warm IC, and Shared-locals
# (closure-creating) variadic bodies.

$side = 0

class T
  def opt_lit(a, b = 1, c = :sym, d = "s", e = nil, f = true)
    [a, b, c, d, e, f]
  end

  def opt_chain(a, b = a + 1, c = b * 2)
    [a, b, c]
  end

  def opt_side(a, b = ($side += 1))
    [a, b, $side]
  end

  def helper = 42
  def opt_call(a, b = helper)
    [a, b]
  end

  def splat(*xs)
    xs << :mutated # must not leak into the next call
    xs
  end

  def req_splat(a, b, *rest)
    [a, b, rest]
  end

  def mid(a, *b, c)
    [a, b, c]
  end

  def mid2(a, *b, c, d)
    [a, b, c, d]
  end

  def optpost(a = :defa, b)
    [a, b]
  end

  def optsplat(a, b = :bee, *rest)
    [a, b, rest]
  end

  def blk_param(a, &blk)
    [a, blk.nil?]
  end

  def splat_blk(*xs, &blk)
    [xs, blk.nil?, block_given?]
  end

  def kw_opt(a, k: :kay)
    [a, k]
  end

  def kw_req(a, k:)
    [a, k]
  end

  def kw_rest(a, **opts)
    [a, opts]
  end

  def kw_computed(a, k: a * 2)
    [a, k]
  end

  def trail_hash(a, *rest)
    [a, rest]
  end

  def call_private_opt(x) = private_opt(x)
  def call_private_opt0 = private_opt
  private def private_opt(a = :priv)
    [:private_opt, a]
  end

  protected def protected_opt(a = :prot)
    [:protected_opt, a]
  end
  def call_protected_opt = protected_opt

  def chain(n)
    inner(n) + inner(n, 10)
  end

  def inner(a, b = 100)
    a + b
  end
end

t = T.new

3.times do |i|
  puts "== iter #{i} =="
  puts t.opt_lit(0).inspect
  puts t.opt_lit(0, 9).inspect
  puts t.opt_lit(0, 9, :x, "y", 1, false).inspect
  puts t.opt_chain(5).inspect
  puts t.opt_chain(5, 100).inspect
  puts t.opt_chain(5, 100, 1000).inspect
  puts t.opt_side(:a).inspect
  puts t.opt_side(:a, :given).inspect # side-effect must NOT fire
  puts t.opt_call(7).inspect
  puts t.opt_call(7, 8).inspect
  a1 = t.splat(1, 2, 3)
  a2 = t.splat(1, 2, 3)
  puts (a1.equal?(a2)).inspect # fresh array per call
  puts a1.inspect
  puts t.splat.inspect
  puts t.req_splat(1, 2).inspect
  puts t.req_splat(1, 2, 3, 4, 5).inspect
  puts t.mid(1, 2).inspect
  puts t.mid(1, 2, 3, 4, 5).inspect
  puts t.mid2(1, 2, 3).inspect
  puts t.mid2(1, 2, 3, 4, 5, 6).inspect
  puts t.optpost(:bee).inspect
  puts t.optpost(:x, :y).inspect
  puts t.optsplat(1).inspect
  puts t.optsplat(1, 2).inspect
  puts t.optsplat(1, 2, 3, 4).inspect
  puts t.blk_param(1).inspect
  puts t.splat_blk(1, 2).inspect
  puts t.kw_opt(1).inspect
  puts t.kw_opt(1, k: :given).inspect
  puts t.kw_req(1, k: 2).inspect
  puts t.kw_rest(1, x: 1, y: 2).inspect
  puts t.kw_rest(1).inspect
  puts t.kw_computed(3).inspect
  puts t.kw_computed(3, k: :kk).inspect
  puts t.trail_hash(1, { x: 1 }).inspect
  puts t.call_private_opt(:v).inspect
  puts t.call_private_opt0.inspect
  puts t.call_protected_opt.inspect
  puts t.chain(1).inspect
  begin
    t.private_opt
  rescue NoMethodError => e
    puts "NoMethodError:#{e.message[0, 40]}"
  end
end

# ArgumentError messages must stay byte-identical on a WARM call site.
def argerr
  yield
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end

3.times do
  argerr { t.opt_chain }             # 0 for 1..3
  argerr { t.opt_chain(1, 2, 3, 4) } # 4 for 1..3
  argerr { t.req_splat(1) }          # 1 for 2+
  argerr { t.mid(1) }                # 1 for 2+
  argerr { t.optpost }               # 0 for 1..2
  argerr { t.kw_req(1) }             # missing keyword
end

# Method redefinition after a warm IC: the same call site must re-bind.
class R
  def m(a, b = :old)
    [a, b]
  end
end
r = R.new
site = ->(x) { r.m(x) }
puts site.call(1).inspect
puts site.call(2).inspect
class R
  def m(a, b = :new, c = :extra)
    [a, b, c]
  end
end
puts site.call(3).inspect

# Subclass override after a warm IC (polymorphic site).
class Base
  def poly(a, *rest) = [:base, a, rest]
end

class Sub < Base
  def poly(a, b = :sub_default) = [:sub, a, b]
end
objs = [Base.new, Sub.new, Base.new, Sub.new]
2.times do
  objs.each { |o| puts o.poly(1).inspect }
  objs.each { |o| puts o.poly(1, 2).inspect }
end

# send to a private variadic.
3.times { puts t.send(:private_opt).inspect }
3.times { puts t.send(:private_opt, :sent).inspect }

# Variadic method whose body creates a block (creates_block => Shared
# locals path).
class C
  def cb(a, b = 2, *rest)
    f = -> { [a, b, rest] }
    f.call
  end
end
c = C.new
3.times { puts c.cb(1).inspect; puts c.cb(1, 9, 8, 7).inspect }

# Long tail through the splat.
3.times { puts t.splat(*(1..20).to_a).length }

# super from a variadic subclass method.
class SupA
  def s(a, b = :sup_b)
    [:sup, a, b]
  end
end

class SupB < SupA
  def s(a, b = :subclass_b)
    super(a) + [b]
  end
end
sb = SupB.new
3.times { puts sb.s(1).inspect; puts sb.s(1, 2).inspect }

puts "side total: #{$side}"
