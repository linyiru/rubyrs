# Dispatch-campaign P1: closure-backed (define_method) methods are
# served by the explicit-recv / self-recv monomorphic IC fast paths
# (try_invoke_closure_method_from_stack) instead of falling through
# the whole do_call slow cascade. This fixture pins the serve's
# semantics against CRuby: capture sharing, strict arity, method_gen
# invalidation on redefinition, visibility asymmetry, super /
# __method__, dm-share re-entrancy, and the universal-arm names
# (`nil?` / `!`) whose define_method overrides must win like their
# `def` twins.

# Explicit-recv hot loop: capture write-back accumulates across calls.
class Bumper
  state = 0
  define_method(:bump) { state = state + 1; state }
  define_method(:peek) { state }
end
b = Bumper.new
100.times { b.bump }
puts "#{b.bump} #{b.peek}"

# Args bind stack-direct (1-arg / 2-arg) + capture read.
class Adder
  base = 100
  define_method(:add) { |x| base + x }
  define_method(:mac) { |x, y| base + x * y }
end
a = Adder.new
puts "#{a.add(5)} #{a.mac(2, 3)}"

# Arity misses decline to the canonical ArgumentError.
begin
  a.add(1, 2)
rescue ArgumentError => e
  puts e.message
end
begin
  a.add
rescue ArgumentError => e
  puts e.message
end

# Redefinition mid-hot-loop: the IC serve revalidates by method_gen.
class Redef
  define_method(:v) { 1 }
end
r = Redef.new
out = []
4.times do |i|
  out << r.v
  Redef.define_method(:v) { 2 } if i == 1
end
puts out.inspect

# `def` replacing `define_method` mid-loop (closure -> proto swap).
class Swap
  define_method(:w) { :dm }
end
s = Swap.new
out = []
4.times do |i|
  out << s.w
  Swap.class_eval { def w; :def; end } if i == 1
end
puts out.inspect

# Visibility asymmetry: implicit-self serves private; explicit raises.
class Vis
  private define_method(:secret) { 42 }
  def call_secret; secret; end
end
v = Vis.new
puts v.call_secret
begin
  v.secret
rescue NoMethodError => e
  puts e.class
end

# super + __method__ inside the served body (invoked_name aux stamp).
class NameBase
  def name_probe; "base"; end
end
class NameSub < NameBase
  define_method(:name_probe) { "#{__method__}/#{super()}" }
end
puts NameSub.new.name_probe

# Recursion through a served dm method (dm_share re-entrancy gate:
# inner activations must not clobber the outer's shared cell).
class Fact
  define_method(:fact) { |n| n <= 1 ? 1 : n * fact(n - 1) }
end
puts Fact.new.fact(6)

# Body-local freshness per call on the shared cell (own-region reset).
class Fresh
  define_method(:fresh) { |x| t = nil; t ||= x; t }
end
f = Fresh.new
puts "#{f.fresh(1)} #{f.fresh(2)}"

# Complex shapes decline to the canonical binder (optionals / rest /
# block-arg / kwargs) — still correct, just not IC-served.
class Complexes
  define_method(:opt) { |p, q = 10| p + q }
  define_method(:rest) { |*xs| xs.sum }
  define_method(:blk) { |&blk| blk.call(3) }
  define_method(:kw) { |**kw| kw[:k].to_i }
end
c = Complexes.new
puts "#{c.opt(1)} #{c.opt(1, 2)} #{c.rest(1, 2, 3)} #{c.blk { |x| x * 2 }} #{c.kw(k: 9)}"

# Universal-arm names: define_method overrides of nil? / ! win over
# the built-in truthiness arms, matching their `def` twins (CRuby).
class Univ
  define_method(:nil?) { true }
  define_method(:!) { :bang }
end
u = Univ.new
puts "#{u.nil?} #{!u}"

# Copy path (outer chain / non-canonical cell): define_method created
# inside a class method whose locals it captures at depth.
class Depth
  def self.make(tag)
    prefix = "p"
    define_method(:tagged) { |x| [prefix, tag, x].join("-") }
  end
  make("t1")
end
d = Depth.new
puts "#{d.tagged('a')} #{d.tagged('b')}"

# Two dm methods sharing one captured cell with the class body.
class Shared
  items = []
  define_method(:push_item) { |x| items << x; items.size }
  define_method(:all) { items }
end
sh = Shared.new
sh.push_item(1)
sh.push_item(2)
puts sh.all.inspect

# send / public_send re-aims land on the same serve.
puts "#{b.send(:bump)} #{b.public_send(:peek)}"

# Destructuring block param binds like the canonical binder.
class Pairs
  define_method(:pair) { |(x, y)| [y, x] }
end
puts Pairs.new.pair([1, 2]).inspect

# Module-defined dm method reached through include (chain lookup).
module Mixin
  define_method(:from_mod) { :mod }
end
class Host; include Mixin; end
puts Host.new.from_mod

# Protected dm method: kin call serves, outside call raises.
class Kin
  protected define_method(:prot) { 7 }
  def probe(other); other.prot; end
end
k = Kin.new
puts k.probe(Kin.new)
begin
  k.prot
rescue NoMethodError => e
  puts e.class
end
