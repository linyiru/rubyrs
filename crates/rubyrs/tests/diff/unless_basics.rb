# Differential fixture for `unless` in all three forms. Both rubyrs
# and CRuby must produce identical stdout.

# 1. Statement form, no else
def warn_if_negative(n)
  unless n >= 0
    puts "negative: #{n}"
  end
end
warn_if_negative(-1)
warn_if_negative(0)
warn_if_negative(5)

# 2. Statement form with else
def classify(n)
  unless n.nil?
    puts "got #{n}"
  else
    puts "nil"
  end
end
classify(nil)
classify(42)

# 3. Modifier form
def squeak(verbose)
  puts "loud" unless verbose
  puts "quiet" if verbose
end
squeak(true)
squeak(false)

# 4. Modifier form using nil-as-falsy
x = nil
puts "x was nil" unless x
y = 0       # 0 is truthy in Ruby
puts "y was nil" unless y

# 5. unless wrapping a method call
puts "list empty" unless [1, 2].length > 0
puts "list non-empty" unless [].length > 0

# 6. Returns nil when condition is truthy and no else
def returns_under_unless(c)
  unless c
    "ran body"
  end
end
r1 = returns_under_unless(true)
r2 = returns_under_unless(false)
puts r1.nil? ? "nil" : r1
puts r2.nil? ? "nil" : r2

# 7. Nested with if
def describe(n)
  unless n.nil?
    if n > 0
      "positive #{n}"
    elsif n < 0
      "negative #{n}"
    else
      "zero"
    end
  else
    "nothing"
  end
end
puts describe(5)
puts describe(-3)
puts describe(0)
puts describe(nil)
