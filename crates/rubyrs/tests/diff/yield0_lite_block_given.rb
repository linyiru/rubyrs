# Zero-arg `yield` in hot loops (the tier-2 LITE-BLOCK serve when it is
# on, the framed ib0 binder otherwise) and bare `block_given?` resolved
# ahead of the implicit-self IC. Each shape runs many times so the
# callee and the block both get hot.
class Y
  def y; yield; end
  def y2; [yield, yield]; end
  def bgy; block_given? ? yield : :none; end
end
o = Y.new

acc = 0
200.times { o.y { acc += 1 } }
p acc

# 1-param block given no args binds nil every time.
p (1..50).map { o.y { |x| x } }.uniq
p (1..50).map { o.y { |x| x = 3 unless x; x } }.uniq
p (1..50).map { o.y2 { |x| x } }.uniq

# Body locals are fresh each invocation.
p (1..50).map { o.y { t = (t || 0) + 1; t } }.uniq

# Closures captured inside the served block see their own iteration.
ps = []
5.times { |i| o.y { ps << -> { i * 2 } } }
p ps.map(&:call)

# Control flow out of the served block, repeated.
p (1..40).map { |i| o.y { next i * 2 if i.even?; i } }.sum
p (1..40).map { |i| o.y { break :b if i > 0 } }.uniq
def ret_y(o); 30.times { o.y { return :ret } }; :no; end
p ret_y(o)
errs = 0
40.times { begin; o.y { raise "x" }; rescue => e; errs += 1 if e.message == "x"; end }
p errs

# Nested yield through a served block, and a block that yields itself.
def outer(o); o.y { yield + 1 }; end
p (1..30).map { outer(o) { 41 } }.uniq
def deep(o, n) = n.zero? ? o.y { :bottom } : o.y { deep(o, n - 1) }
p deep(o, 20)

# Lambdas: 0-param served, 1-param raises.
l0 = -> { :lam }
p (1..30).map { o.y(&l0) }.uniq
l1 = ->(a) { a }
n = 0
30.times { begin; o.y(&l1); rescue ArgumentError; n += 1; end }
p n

# block_given? with and without a block, in hot loops.
p (1..40).map { |i| i.even? ? o.bgy { :blk } : o.bgy }.tally

# Class-self bare block_given?.
class K
  def self.m; block_given? ? yield : :none; end
end
p (1..30).map { |i| i.odd? ? K.m { :c } : K.m }.tally

# User overrides of block_given? must still win.
class Ov
  def block_given? = :overridden
  def t; block_given?; end
end
p (1..20).map { Ov.new.t { } }.uniq

module BG
  def block_given? = :from_module
end
class Late
  def t; block_given?; end
end
lt = Late.new
r1 = (1..20).map { lt.t { } }.uniq
Late.include(BG)
r2 = (1..20).map { lt.t { } }.uniq
p [r1, r2]

s = Late.new
def s.block_given? = :singleton
p (1..20).map { s.t }.uniq
class Priv
  def t; block_given?; end
  private def block_given? = :private_override
end
p (1..20).map { Priv.new.t { } }.uniq

# Class-self overrides: own singleton, inherited singleton, module
# singleton, and one defined after the call site is already hot.
class KO
  def self.block_given? = :override
  def self.m; block_given?; end
end
p (1..20).map { |i| i.odd? ? KO.m { } : KO.m }.uniq
class KP; def self.block_given? = :parent; end
class KC < KP; def self.m; block_given?; end; end
p (1..20).map { KC.m { } }.uniq
module KM; def self.block_given? = :mod; def self.m; block_given?; end; end
p (1..20).map { KM.m { } }.uniq
class KL; def self.m; block_given?; end; end
r1 = (1..20).map { KL.m { } }.uniq
def KL.block_given? = :late
r2 = (1..20).map { KL.m { } }.uniq
p [r1, r2]

# block_given? inside a zero-arg-yielded block reads the enclosing
# method's block, not the yielding method's.
def lm0(o) = o.y { block_given? }
def lm1(o) = o.y2 { block_given? }
p (1..30).map { [lm0(o), lm0(o) { }, lm1(o), lm1(o) { }] }.uniq

# Instance methods on the class object's own chain override a bare
# block_given? in a class method (Module, then Class, then Object).
class CA; def self.m; block_given?; end; end
module CB; def self.m; block_given?; end; end
p (1..20).map { [CA.m { }, CB.m { }] }.uniq
class Module; def block_given? = :module_inst; end
p (1..20).map { [CA.m { }, CB.m { }] }.uniq
class Class; def block_given? = :class_inst; end
p (1..20).map { [CA.m { }, CB.m { }] }.uniq
class Module; remove_method :block_given?; end
class Class; remove_method :block_given?; end
p (1..20).map { [CA.m { }, CB.m { }] }.uniq
class Object; def block_given? = :obj_inst; end
p (1..20).map { [CA.m { }, CB.m { }, o.bgy { }] }.uniq
