# Tier-2 frameless ("lite") callers serving a cached fixed-arity
# method_missing (#391). Under RUBYRS_JIT_TIER2 the small caller bodies
# below run frameless; the interpreter must observe the same results.

class Fx
  def method_missing(name) = "Fx(#{name})"
end
class Two
  def method_missing(name, a) = "Two(#{name}, #{a})"
end
class Deep
  def method_missing(name) = helper(name)
  def helper(n) = "Deep(#{n})"
end
class Raiser
  def method_missing(name)
    raise ArgumentError, "raised in mm for #{name}" if $raise
    "Raiser(#{name})"
  end
end
class Sup
  def method_missing(name)
    return "Sup(#{name})" if name == :ok
    super
  end
end
class Where
  def method_missing(name) = "#{name} from #{__method__}, depth ok #{caller_locations(1, 1).size == 1}"
end
class SelfFx
  def method_missing(name, *rest) = rest.empty? ? "SelfFx(#{name})" : "SelfFx(#{name}, #{rest.inspect})"
  def bare = nope
  def bare_arg(x) = nope(x)
end
class SelfFixed
  def method_missing(name) = "SelfFixed(#{name})"
  def bare = nope
  def bare_arg(x) = nope(x)
end

def ex0(o) = o.nope
def ex1(o, x) = o.nope(x)
def ex_ok(o) = o.ok

fx = Fx.new; two = Two.new; deep = Deep.new; ra = Raiser.new; sup = Sup.new
50.times do |i|
  r = [ex0(fx), ex1(two, i), ex0(deep), ex0(ra), ex_ok(sup), SelfFixed.new.bare, SelfFx.new.bare_arg(i)]
  puts r.join(" ") if i % 10 == 0
end

# Arity mismatch on a warmed site: ArgumentError from method_missing.
begin
  ex1(fx, 1)
rescue ArgumentError
  puts "ArgumentError fx"
end
begin
  SelfFixed.new.bare_arg(1)
rescue ArgumentError
  puts "ArgumentError self"
end
begin
  ex0(two)
rescue ArgumentError
  puts "ArgumentError two"
end

# Raise from inside a frameless-served method_missing.
$raise = true
3.times do
  begin
    ex0(ra)
  rescue ArgumentError => e
    puts e.message
  end
end
$raise = false
puts ex0(ra)

# super to BasicObject#method_missing from a served method_missing.
3.times do
  begin
    ex0(sup)
  rescue NoMethodError
    puts "NoMethodError sup"
  end
end

# __method__ and the caller frames seen from inside method_missing.
3.times { puts ex0(Where.new) }

# Redefine to a rest shape, then back to fixed, on the warmed sites.
class Fx
  def method_missing(name, *args) = "Fx rest(#{name}, #{args.size})"
end
3.times { puts ex0(fx) }
class Fx
  def method_missing(name) = "Fx again(#{name})"
end
3.times { puts ex0(fx) }

# A real method appears: the site must stop serving method_missing.
class Fx
  def nope = "real nope"
end
3.times { puts ex0(fx) }

# A stack of pending frameless callers above a served method_missing.
def lvl3(o) = o.nope
def lvl2(o) = lvl3(o) + "!"
def lvl1(o) = lvl2(o) + "?"
50.times { |i| s = lvl1(deep); puts s if i % 25 == 0 }

# Bodies simple enough to run frameless themselves: the served
# method_missing is entered as a lite->lite chain.
class Lite0
  def method_missing(name) = name
end
class Lite1
  def method_missing(name, a) = a + 1
end
class LiteChain
  def method_missing(name, a) = inc(a)
  def inc(a) = a + 10
end
class LiteSelf
  def method_missing(name, a) = a * 2
  def go(x) = twice(x)
end
def lx0(o) = o.anything
def lx1(o, a) = o.plus(a)
l0 = Lite0.new; l1 = Lite1.new; lc = LiteChain.new; ls = LiteSelf.new
acc = 0
200.times { |i| acc += lx1(l1, i) + lx1(lc, i) + ls.go(i) }
puts acc
puts [lx0(l0), lx0(l0)].inspect
# A raise inside the frameless method_missing body (nil + 1).
2.times do
  begin
    lx1(l1, nil)
  rescue NoMethodError => e
    puts e.class
  end
end
puts lx1(l1, 1.5)
