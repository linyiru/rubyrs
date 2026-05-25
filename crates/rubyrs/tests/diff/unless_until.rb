# `unless` and `until` — both keyword and modifier forms.
# Desugared at AST time: `unless` swaps an `if`'s branches;
# `until` becomes a `while !cond`.

# Block-form unless without else.
x = 5
unless x > 10
  puts "small"
end

# Block-form with else — both arms run depending on cond.
unless x > 10
  puts "small branch"
else
  puts "big branch"
end

unless x < 0
  puts "non-negative"
else
  puts "negative"
end

# Modifier form — single expression.
puts "ok" unless x == 0
puts "skip" unless x > 0

n = nil
puts "still here" unless n

# `unless` returning a value (last branch result).
y = unless x > 10
      "small"
    else
      "big"
    end
puts y

z = unless x == 0
      "non-zero"
    end
puts z

# `unless` with no else and false predicate → nil.
result = unless true
           "never"
         end
puts result.inspect

# Block-form until.
i = 0
until i >= 5
  puts i
  i += 1
end

# Modifier-form until.
j = 0
puts j until (j += 1) > 3

# until with a false starting condition.
k = 10
puts "would have run" until k > 5
puts "(but didn't)"

# until as method-call argument context.
def countdown(n)
  result = []
  until n == 0
    result << n
    n -= 1
  end
  result
end
p countdown(5)

# Nested unless / until.
[1, 2, 3, 4, 5].each do |v|
  unless v.even?
    puts "odd: #{v}"
  end
end

# unless / until inside a class method.
class Validator
  def initialize(value)
    @value = value
  end
  def safe?
    !@value.nil? && @value > 0
  end
  def warn
    puts "danger" unless safe?
  end
end

Validator.new(-1).warn
Validator.new(10).warn
Validator.new(nil).warn

# until in a counter loop returning the count.
def first_pow2_over(n)
  v = 1
  count = 0
  until v > n
    v *= 2
    count += 1
  end
  count
end
puts first_pow2_over(100)
puts first_pow2_over(1000)
