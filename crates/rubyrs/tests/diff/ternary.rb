# Ternary `cond ? then : else` — syntactic sugar for if/else.
# Prism parses it as an IfNode and our existing If translation
# already handles the shape. This fixture pins the contract so
# accidental regressions in the If path get caught here.

# Basic.
x = 5
puts (x > 0 ? "pos" : "non-pos")
puts (x.even? ? "even" : "odd")

# Both branches as expressions.
puts (1 + 1 == 2 ? "math works" : "broken")
puts ("hi".length > 0 ? "has chars" : "empty")

# Nil / false are falsy.
puts (nil ? "truthy" : "falsy")
puts (false ? "truthy" : "falsy")
puts (0 ? "truthy" : "falsy")        # 0 is truthy in Ruby
puts ("" ? "truthy" : "falsy")       # "" is truthy
puts ([] ? "truthy" : "falsy")       # [] is truthy

# Assignment from ternary.
n = 7
sign = n >= 0 ? "+" : "-"
puts sign

# Nested ternaries.
def grade(score)
  score >= 90 ? "A" : score >= 80 ? "B" : score >= 70 ? "C" : "F"
end
puts grade(95)
puts grade(85)
puts grade(75)
puts grade(50)

# Inside a method.
class Bounded
  def initialize(min, max)
    @min = min
    @max = max
  end
  def clamp(x)
    x < @min ? @min : (x > @max ? @max : x)
  end
end

b = Bounded.new(0, 100)
puts b.clamp(-5)
puts b.clamp(50)
puts b.clamp(200)

# Inside a block.
nums = [1, -2, 3, -4, 5]
abs = nums.map { |n| n < 0 ? -n : n }
p abs

# Used as a method-call argument.
def echo(s); s; end
puts echo(true ? "yes" : "no")

# Used in interpolation.
status = :ok
puts "status: #{status == :ok ? "good" : "bad"}"
