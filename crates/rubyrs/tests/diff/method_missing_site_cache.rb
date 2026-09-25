# Call-site method_missing inline cache (#391): a site that has warmed up
# on a method_missing miss must notice every change that alters the miss.

class A
  def method_missing(name, *args)
    "A#mm(#{name}, #{args.inspect})"
  end
  def respond_to_missing?(n, p = false) = true
end

def call_nope(o) = o.nope
def call_two(o) = o.two(1, 2)
def call_zero_args(o) = o.zed

a = A.new
3.times { puts call_nope(a) }
3.times { puts call_two(a) }

# Redefine method_missing after the site warmed up.
class A
  def method_missing(name, *args)
    "A#mm2(#{name}, #{args.size})"
  end
end
puts call_nope(a)
puts call_two(a)

# Define the real method: the site must stop routing to method_missing.
class A
  def nope = "real nope"
end
puts call_nope(a)

# Remove it again: back to method_missing.
class A
  remove_method :nope
end
puts call_nope(a)

# Remove method_missing itself: NoMethodError.
class A
  remove_method :method_missing
end
begin
  call_nope(a)
rescue NoMethodError => e
  puts e.class
end

# Polymorphic site: several classes with different method_missing shapes.
class B
  def method_missing(name) = "B(#{name})"
end
class C
  def method_missing(name, *args, &blk) = "C(#{name}, #{args.size}, #{blk.inspect})"
end
class D
  define_method(:method_missing) { |name, *args| "D(#{name})" }
end
class E
  def method_missing(name, *args, **kw) = "E(#{name}, #{args.inspect}, #{kw.inspect})"
end
class F < C
end
class G < C
  def method_missing(name, *args)
    "G then " + super
  end
end
class H
  private def method_missing(name, *args) = "H private mm(#{name})"
end
objs = [B.new, C.new, D.new, E.new, F.new, G.new, H.new]
3.times { objs.each { |o| puts call_zero_args(o) } }
objs.each do |o|
  begin
    puts call_two(o)
  rescue ArgumentError => e
    puts "#{o.class}: ArgumentError #{e.message}"
  end
end

# Per-object singleton method_missing on a warmed class.
b2 = B.new
def b2.method_missing(name) = "b2 singleton(#{name})"
puts call_zero_args(b2)
puts call_zero_args(B.new)

# extend a module that defines method_missing.
module M
  def method_missing(name, *args) = "M(#{name}) then #{super}"
end
b3 = B.new
b3.extend(M)
puts call_zero_args(b3)

# A private method is a miss too, but only from outside.
class P
  def method_missing(name, *args) = "P mm(#{name})"
  private def zed = "private zed"
  def inside = zed
end
p1 = P.new
3.times { puts call_zero_args(p1) }
puts p1.inside
class P
  public :zed
end
puts call_zero_args(p1)

# method_missing that raises, and __method__ inside it.
class R
  def method_missing(name, *args)
    raise ArgumentError, "no #{name}" if name == :zed && args.empty? && $raise
    "R #{__method__} #{name}"
  end
end
r = R.new
2.times { puts call_zero_args(r) }
$raise = true
begin
  call_zero_args(r)
rescue ArgumentError => e
  puts "rescued: #{e.message}"
end

# super to BasicObject#method_missing from a warmed site.
class S
  def method_missing(name, *args)
    return "S handled #{name}" if name == :nope
    super
  end
end
s = S.new
2.times { puts call_nope(s) }
begin
  call_zero_args(s)
rescue NoMethodError => e
  puts e.class
end

# undef on a warmed class routes the undef'd name to method_missing.
class U
  def method_missing(name, *args) = "U mm(#{name})"
  def zed = "U zed"
end
u = U.new
2.times { puts call_zero_args(u) }
class U
  undef_method :zed
end
2.times { puts call_zero_args(u) }

# A site as wide as the cache: each missing class holds one way, which also
# answers the regular lookup negatively. Then give one class a real method,
# another a private one, and check every way still answers correctly.
wide = (1..5).map do |i|
  Class.new { define_method(:method_missing) { |name, *args| "W#{i}(#{name})" } }
end
wobjs = wide.map(&:new)
3.times { puts wobjs.map { |o| call_zero_args(o) }.join(" ") }
wide[2].class_eval { def zed = "W3 real zed" }
wide[3].class_eval { private def zed = "W4 private zed" }
2.times { puts wobjs.map { |o| call_zero_args(o) }.join(" ") }

# Hot loop over one site.
acc = 0
k = A.new
class A
  def method_missing(name, *args) = args.sum
end
2000.times { |i| acc += k.add(i, 1) }
puts acc
