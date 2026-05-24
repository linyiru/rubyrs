# Plain `return` from inside a method
def first_positive(arr)
  i = 0
  while i < arr.length
    if arr[i] > 0
      return arr[i]
    end
    i = i + 1
  end
  -1
end

puts first_positive([-3, -1, 0, 4, -2, 9])
puts first_positive([-1, -2, -3])

# `return` with no value
def returns_nil
  return
  puts "unreached"
end

x = returns_nil
puts x.nil?

# `break` from inside a block — exits .each early with the value
found = [1, 2, 3, 4, 5].each do |n|
  break n if n > 2
end
puts found

# `break` without arg returns nil
result = [1, 2, 3].each { |x| break if x == 2 }
puts result.nil?

# `next` from inside a block — skips this iteration, continues
sum = 0
[1, 2, 3, 4, 5].each do |n|
  next if n == 3
  sum = sum + n
end
puts sum

# `break` with map: returns the break value (not the partial array)
r = [1, 2, 3, 4].map { |x| break "early" if x == 3; x * 10 }
puts r

# 3.times with break
counted = 5.times { |i| break i if i == 2 }
puts counted