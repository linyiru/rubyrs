# Literal-Symbol keyword calls (`f(a: 1)`) bind without a Hash when the
# target is a plain `def` with keyword params (Op::CallKwLit). Every
# other shape rebuilds the Hash and takes the general path. Both halves
# must match CRuby, including error messages and default evaluation.

class K
  def req(a:, b:) = [a, b]
  def opt(a: 1, b: "s") = [a, b]
  def mixed(x, y = x + 1, *rest, z, k:, j: k * 2, &blk) = [x, y, rest, z, k, j, blk.nil?]
  def post(a, b, k: 0) = [a, b, k]
  def computed(a:, b: a + 10, c: b.to_s) = [a, b, c]
  def str_default(s: "x") = (s << "y")
  def plain(h) = h
  def splat(*a) = a
  def rest_kw(a:, **o) = [a, o]
  def given(a: (p :evaluated; 5)) = a
  def self_call = req(b: 2, a: 1)
  def self_private = hidden(v: 3)
  def many(k1: 1, k2: 2, k3: 3, k4: 4, k5: 5, k6: 6, k7: 7, k8: 8, k9: 9, k10: 10,
           k11: 11, k12: 12, k13: 13, k14: 14, k15: 15, k16: 16, k17: 17, k18: 18) = [k1, k9, k17, k18]
  def nested(a:) = a
  def raises(a:) = raise(ArgumentError, "boom #{a}")
  def blocky(k:) = block_given? ? yield(k) : :noblk
  protected def prot(a:) = a
  private def hidden(v:) = v * 2
end

class G
  def method_missing(name, *args, **kw) = [name, args, kw]
  def respond_to_missing?(n, p = false) = true
end

def try
  yield
rescue => e
  "#{e.class}: #{e.message}"
end

def run
  k = K.new
  p k.req(a: 1, b: 2)
  p k.req(b: 2, a: 1)
  p k.opt(a: 9)
  p k.opt(b: :z)
  p k.opt
  p k.mixed(1, 2, k: 3)
  p k.mixed(1, 2, 3, 4, 5, k: 6, j: 7)
  p k.post(1, 2, k: 3)
  p k.computed(a: 1)
  p k.computed(a: 1, c: :cc)
  p k.computed(b: 2, a: 1)
  3.times { p k.str_default }
  p k.str_default(s: +"q")
  p k.plain(a: 1, b: 2)
  p k.splat(1, a: 2)
  p k.rest_kw(a: 1, b: 2, c: 3)
  p k.rest_kw(a: 1)
  p k.given
  p k.given(a: 1)
  p k.self_call
  p k.self_private
  p k.many(k9: :nine, k18: :last)
  p k.nested(a: k.nested(a: k.req(a: 1, b: 2)))
  p k.blocky(k: 4) { |v| v * 10 }
  p k.blocky(k: 4)
  p G.new.ghost_x(1, a: 2)
  p k.send(:req, a: 1, b: 2)
  p k.public_send(:opt, a: 5)
  p k.method(:req).call(a: 3, b: 4)
  # errors come from the general path
  p try { k.req(a: 1) }
  p try { k.req }
  p try { k.req(a: 1, b: 2, c: 3) }
  p try { k.req(a: 1, c: 3) }
  p try { k.opt(zz: 1) }
  p try { k.post(1, k: 2) }
  p try { k.prot(a: 1) }
  p try { k.hidden(v: 1) }
  p((k.nope(a: 1) rescue $!.class))
  p try { k.raises(a: 7) }
  p try { k.req(a: raise("in arg"), b: 2) }
  # builtins and other receivers
  p 25.round(-1, half: :even)
  p 2.5.round(half: :up)
  p Integer("12", exception: false)
  p Integer("zz", exception: false)
  st = Struct.new(:a, :b, keyword_init: true)
  p st.new(a: 1, b: 2).to_a
  p({x: 1}.merge(y: 2))
  p [3, 1, 2].sort_by { |v| v }.each_slice(2).to_a
  p "a-b".split("-", 2)
  # class-method and module-function receivers
  p K.new.then { |o| o.opt(a: :then) }
  def K.cm(a:, b: 2) = [a, b]
  p K.cm(a: 1)
  obj = Object.new
  def obj.sing(q:) = q + 1
  p obj.sing(q: 1)
  # redefinition must invalidate the call-site cache
  r = []
  2.times do |i|
    r << k.opt(a: i)
    K.class_eval { def opt(a: 0, b: 0) = [:redefined, a, b] } if i == 0
  end
  p r
  # a method that loses its kwargs
  K.class_eval { def req(*args) = [:positional, args] }
  p k.req(a: 1, b: 2)
  # inheritance and super with keywords
  base = Class.new { def w(a:, b: 1) = [a, b] }
  sub = Class.new(base) { def w(a:, b: 5) = [super, :sub] }
  p sub.new.w(a: 0)
  p sub.new.w(a: 0, b: 9)
end
run

# top-level (main self) and a loop hot enough for the JITs
def top(a:, b: 1) = a + b
p top(a: 1)
def hot(o)
  s = 0
  i = 0
  while i < 2000
    s += o.req2(x: i, y: 1)
    i += 1
  end
  s
end
class K
  def req2(x:, y:) = x + y
end
p hot(K.new)

# A direct serve inside `eval` (an outer synchronous call) must not
# inherit the positional-Hash flag: zsuper forwards the keywords.
class ZB
  def w(a:, b: 1) = [a, b]
  def z(**o) = o
end
class ZC < ZB
  def w(a:, b: 2) = [super, :c]
  def z(a:) = super
  def fwd(a:) = z(a: a)
end
ZO = ZC.new
p eval("ZO.w(a: 1)")
p eval("ZO.w(a: 1, b: 3)")
p eval("ZO.z(a: 5)")
p eval("ZO.fwd(a: 6)")
p [1].map { |x| eval("ZO.w(a: 4)") }
p ZO.instance_eval { w(a: 9) }
