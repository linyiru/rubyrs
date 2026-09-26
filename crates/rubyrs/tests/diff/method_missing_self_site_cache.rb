# Call-site method_missing inline cache for bare implicit-self calls (#391):
# `nope` inside an instance method, on a site that has warmed up on the miss.

class A
  def method_missing(name, *args)
    "A#mm(#{name}, #{args.inspect})"
  end
  def respond_to_missing?(n, p = false) = true
  def bare_zero = nope
  def bare_two = two(1, 2)
  def bare_splat(*a) = sp(*a)
end

a = A.new
3.times { puts a.bare_zero }
3.times { puts a.bare_two }
3.times { |i| puts a.bare_splat(*([i] * i)) }

# Redefine method_missing after the site warmed up.
class A
  def method_missing(name, *args) = "A#mm2(#{name}, #{args.size})"
end
puts a.bare_zero
puts a.bare_two

# Define the real method (private is fine for a bare call), then remove it.
class A
  private def nope = "private real nope"
end
puts a.bare_zero
class A
  remove_method :nope
end
puts a.bare_zero

# Remove method_missing itself: NameError from a bare call.
class A
  remove_method :method_missing
end
begin
  a.bare_zero
rescue NameError => e
  puts "missing"
end

# Polymorphic self: subclasses inherit the bare site.
class Base
  def call_zed = zed
  def call_args = zed(1, k: 2)
end
class B < Base
  def method_missing(name) = "B(#{name})"
end
class C < Base
  def method_missing(name, *args, &blk) = "C(#{name}, #{args.size}, #{blk.inspect})"
end
class D < Base
  define_method(:method_missing) { |name, *args| "D(#{name}, #{args.size})" }
end
class E < Base
  def method_missing(name, *args, **kw) = "E(#{name}, #{args.inspect}, #{kw.inspect})"
end
class G < C
  def method_missing(name, *args) = "G then " + super
end
class H < Base
  private def method_missing(name, *args) = "H private mm(#{name})"
end
class W1 < Base
  def method_missing(name, *args) = "W1(#{name})"
end
objs = [B.new, C.new, D.new, E.new, G.new, H.new, W1.new]
3.times { puts objs.map(&:call_zed).join(" ") }
objs.each do |o|
  begin
    puts o.call_args
  rescue ArgumentError => e
    puts "#{o.class}: ArgumentError"
  end
end

# Singleton and extend on a warmed class.
b2 = B.new
def b2.method_missing(name) = "b2 singleton(#{name})"
puts b2.call_zed
puts B.new.call_zed
module M
  def method_missing(name, *args) = "M(#{name}) then #{super}"
end
b3 = B.new
b3.extend(M)
puts b3.call_zed

# A real method appears on one warmed class only.
class D
  def zed = "D real zed"
end
2.times { puts objs.map(&:call_zed).join(" ") }

# __method__, raise, and super to BasicObject#method_missing.
class R
  def method_missing(name, *args)
    raise ArgumentError, "no #{name}" if $raise
    return "R #{__method__} #{name}" if name == :zed
    super
  end
  def go = zed
  def other = nothing_here
end
r = R.new
2.times { puts r.go }
2.times do
  begin
    r.other
  rescue NameError => e
    puts "missing"
  end
end
$raise = true
begin
  r.go
rescue ArgumentError => e
  puts "rescued: #{e.message}"
end

# Kernel names never go through method_missing.
class K
  def method_missing(name, *args) = "K mm(#{name})"
  def go = [format("%d", 1), Integer("2"), frozen?, zed].inspect
end
2.times { puts K.new.go }

# Hot loop over one bare site.
class L
  def method_missing(name, *args) = args.sum
  def run(n)
    acc = 0
    n.times { |i| acc += add(i, 1) }
    acc
  end
end
puts L.new.run(2000)
