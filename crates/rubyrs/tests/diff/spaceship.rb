# Integer
puts(5 <=> 10)        # -1
puts(10 <=> 5)        # 1
puts(5 <=> 5)         # 0

# Float
puts(1.5 <=> 2.5)     # -1
puts(2.5 <=> 1.5)     # 1
puts(1.5 <=> 1.5)     # 0

# Mixed numeric coercion
puts(5 <=> 5.0)       # 0
puts(5 <=> 5.5)       # -1
puts(5.0 <=> 4)       # 1
puts(2.0 <=> 2.0)     # 0

# NaN-involved comparisons → nil
nan = 0.0 / 0.0
puts((nan <=> 1.0).nil?)
puts((1.0 <=> nan).nil?)

# String — lex order
puts("a" <=> "b")     # -1
puts("b" <=> "a")     # 1
puts("foo" <=> "foo") # 0
puts("ab" <=> "abc")  # -1
puts("abc" <=> "ab")  # 1

# Symbol — lex on interned name
puts(:apple <=> :banana)
puts(:banana <=> :apple)
puts(:zz <=> :zz)

# Nil
puts(nil <=> nil)              # 0
puts((nil <=> 0).nil?)         # true
puts((nil <=> "").nil?)        # true

# Cross-type → nil
puts((5 <=> "5").nil?)
puts(("x" <=> 5).nil?)
puts((:foo <=> "foo").nil?)
puts((1.5 <=> :nope).nil?)

# Boolean — Ruby's TrueClass / FalseClass have no <=>; result is nil
puts((false <=> true).nil?)
puts((true <=> false).nil?)
puts((true <=> true).nil?)     # CRuby: nil (no Comparable mixed in)

# User-defined <=> wins over the fallback
class Version
  attr_reader :major, :minor
  def initialize(major, minor)
    @major = major
    @minor = minor
  end
  def <=>(other)
    if @major != other.major
      @major <=> other.major
    else
      @minor <=> other.minor
    end
  end
end

a = Version.new(1, 2)
b = Version.new(1, 3)
c = Version.new(1, 2)
puts(a <=> b)
puts(b <=> a)
puts(a <=> c)

# Object without user-defined <=> — same instance → 0, different → nil
class Box
end
x = Box.new
y = Box.new
puts(x <=> x)              # 0 — same instance
puts((x <=> y).nil?)       # true — different instances
