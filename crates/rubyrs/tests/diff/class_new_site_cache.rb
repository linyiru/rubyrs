# One `Foo.new` call site served many times while the class changes
# under it: every change must be seen by the next call from the same
# site (the per-site `new` cache must never serve a stale decision).

class Pt
  attr_reader :x, :y
  def initialize(x, y); @x = x; @y = y; end
end

class Bare; end

def make_pt(a, b) = Pt.new(a, b)
def make_bare = Bare.new
def make(k, *a) = k.new(*a)

p make_pt(1, 2).x
p make_pt(3, 4).y
# The site returns the new object, not initialize's value.
class Ret; def initialize = 42; end
def make_ret = Ret.new
2.times { p make_ret.class }

# Redefine initialize: the site must pick up the new body.
class Pt
  def initialize(x, y); @x = x * 10; @y = y * 10; end
end
p make_pt(1, 2).x

# Redefine with a different arity.
class Pt
  def initialize(x, y = 5); @x = x; @y = y; end
end
p make_pt(1, 2).y

p make_bare.class
p make_bare.class
# Add initialize to a class that had none.
class Bare
  def initialize; @tag = :late; end
  attr_reader :tag
end
p make_bare.tag

# A singleton `new` defined after the site warmed up.
p make_bare.class
class Bare
  def self.new(*) = :custom_new
end
p make_bare
class << Bare
  remove_method :new
end
p make_bare.tag

# Arity errors keep CRuby's message.
begin
  make(Pt, 1)
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end
class Two; def initialize(a, b) = (@s = a + b); attr_reader :s; end
p make(Two, 1, 2).s
begin
  make(Two, 1)
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end
p make(Two, 3, 4).s

# One polymorphic site over many class shapes.
class H < Hash; end
class A < Array; end
class S < String; end
E = Class.new(StandardError)
St = Struct.new(:a, :b)
Da = Data.define(:q)
class WithBlock; def initialize(&b) = (@r = b ? b.call : :none); attr_reader :r; end
class Opt; def initialize(a = 1, *r, k: 2) = (@v = [a, r, k]); attr_reader :v; end
2.times do
  p make(H).class, make(A, 2, :z), make(S, "hi"), make(E, "boom").message
  p make(St, 1, 2).to_a, make(Da, 7).q
  p make(WithBlock).r, WithBlock.new { :blk }.r
  p make(Opt).v, make(Opt, 5, 6, 7).v, Opt.new(k: 9).v
  p make(Pt, 1, 2).x rescue p $!.class
end

# Subclass inheriting initialize, then overriding it.
class Base; def initialize(n) = (@n = n); attr_reader :n; end
class Kid < Base; end
def make_kid(n) = Kid.new(n)
p make_kid(1).n
class Kid; def initialize(n) = super(n + 100); end
p make_kid(1).n

# Including a module that defines initialize.
module Init; def initialize(*) = (@m = :from_module); end
class Plain2; attr_reader :m; end
def make_plain2 = Plain2.new
p make_plain2.m
class Plain2; include Init; end
p make_plain2.m
# Prepend overrides the class's own initialize.
class Plain3; def initialize = (@w = :own); attr_reader :w; end
def make_plain3 = Plain3.new
p make_plain3.w
module Pre; def initialize = (@w = :prepended); end
class Plain3; prepend Pre; end
p make_plain3.w

# An exception raised inside initialize propagates from the site.
class Boom; def initialize(x) = (raise "bad #{x}" if x); end
def make_boom(x) = Boom.new(x)
p make_boom(nil).class
begin
  make_boom(1)
rescue => e
  puts e.message
end
p make_boom(nil).class

# Many objects through a warm site: all distinct, all initialized.
objs = Array.new(1000) { |i| make_pt(i, -i) }
p objs.map(&:y).sum, objs.uniq(&:object_id).size
