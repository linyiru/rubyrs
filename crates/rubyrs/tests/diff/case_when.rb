# case / when statement. Desugars to nested if/elsif using `===`:
#   case x; when a, b; ...; when c; ...; end
#   → if a === x || b === x; ...; elsif c === x; ...; end
# Without a predicate the when conditions are plain booleans.

def fizzy(n)
  case n
  when 0 then "zero"
  when 1, 2 then "few"
  when 3..10 then "small"
  when 11..100 then "medium"
  when Integer then "big int"
  when String then "string"
  else "other"
  end
end

puts fizzy(0)
puts fizzy(1)
puts fizzy(2)
puts fizzy(5)
puts fizzy(50)
puts fizzy(500)
puts fizzy("hi")
puts fizzy(:foo)
puts fizzy(nil)

# Multi-line when bodies.
def grade(score)
  case score
  when 90..100
    "A"
  when 80...90
    "B"
  when 70...80
    "C"
  else
    "F"
  end
end

puts grade(95)
puts grade(85)
puts grade(75)
puts grade(60)
puts grade(100)
puts grade(80)
puts grade(89)

# Predicate-less (each when is a plain bool).
def sign(n)
  case
  when n > 0 then "positive"
  when n < 0 then "negative"
  else "zero"
  end
end

puts sign(5)
puts sign(-3)
puts sign(0)
puts sign(0.5)

# case with String predicate using string equality.
def lang(s)
  case s
  when "ruby", "python" then "scripting"
  when "rust", "go", "c" then "systems"
  else "other"
  end
end

puts lang("ruby")
puts lang("rust")
puts lang("ada")

# case returning a value.
result = case 2
         when 1 then "one"
         when 2 then "two"
         when 3 then "three"
         end
puts result

# No else, no match → nil.
no_match = case 999
           when 1 then "one"
           when 2 then "two"
           end
puts no_match.inspect

# Class instance check.
def describe(x)
  case x
  when Integer then "int=#{x}"
  when Float then "float=#{x}"
  when String then "str=#{x}"
  when Symbol then "sym=#{x}"
  when Array then "arr len=#{x.length}"
  when Hash then "hash len=#{x.length}"
  when NilClass then "nil"
  when TrueClass, FalseClass then "bool=#{x}"
  else "unknown"
  end
end

puts describe(42)
puts describe(3.14)
puts describe("text")
puts describe(:tag)
puts describe([1, 2, 3])
puts describe({a: 1})
puts describe(nil)
puts describe(true)
puts describe(false)

# Range#=== with Float predicate.
def temp_band(t)
  case t
  when 0.0..10.0 then "cold"
  when 10.0..25.0 then "mild"
  when 25.0..40.0 then "warm"
  else "extreme"
  end
end

puts temp_band(5.0)
puts temp_band(20.0)
puts temp_band(30.0)
puts temp_band(-5.0)
puts temp_band(100.0)

# case inside a class method.
class Token
  def initialize(s)
    @raw = s
  end
  def kind
    case @raw
    when "+", "-", "*", "/" then "op"
    when "(" then "lparen"
    when ")" then "rparen"
    else "ident"
    end
  end
end

puts Token.new("+").kind
puts Token.new("(").kind
puts Token.new("hello").kind
