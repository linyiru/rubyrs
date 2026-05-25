# Basic: return from inside a block exits the enclosing method
def first_positive(arr)
  arr.each do |n|
    return n if n > 0
  end
  nil
end
puts first_positive([-1, -2, 3, 4])
puts first_positive([1, 2, 3])
puts first_positive([-1, -2]).nil?

# Return value of nil propagates correctly
def returns_nil(arr)
  arr.each do |x|
    return nil
  end
  "fallback"
end
puts returns_nil([1, 2, 3]).nil?

# Nested blocks — return exits the outermost (the enclosing method)
def find_pair(arr, target)
  arr.each do |a|
    arr.each do |b|
      return [a, b] if a + b == target
    end
  end
  nil
end
pair = find_pair([1, 2, 5, 8], 10)
puts pair[0]
puts pair[1]
puts find_pair([1, 2, 3], 999).nil?

# Return from inside map / inject — short-circuits the iterator
def first_long(strings)
  strings.map do |s|
    return s if s.length > 5
    s.upcase
  end
end
puts first_long(["hi", "hello", "longword", "foo"])
# When no long string, returns the mapped array
def all_short(strings)
  strings.map do |s|
    return "found short" if s.length < 3
    s
  end
end
puts all_short(["short", "abc", "fine"])

# Return from inside .times
def power_of_two_within(limit)
  result = nil
  10.times do |i|
    return i if (1 << i) >= limit
  end
  result
end
puts power_of_two_within(100)
puts power_of_two_within(1)

# Return interacts correctly with ensure (NOT — documented gap;
# our `return` doesn't run ensure blocks. Skip this case in the
# fixture and document in SUBSET.md.)

# Return with explicit value or implicit nil
def two_paths(cond)
  if cond
    return "yes"
  end
  [1, 2].each do |n|
    return n * 100 if n == 2
  end
end
puts two_paths(true)
puts two_paths(false)

# Implicit (non-`return`) values from a method that uses blocks
# unaffected
def sum(arr)
  total = 0
  arr.each { |n| total = total + n }
  total
end
puts sum([1, 2, 3, 4])

# Return inside a block invoked through `super`
class Base
  def find(arr)
    arr.each do |x|
      return x if x.even?
    end
    nil
  end
end
class Child < Base
  def find(arr)
    "wrapper: " + (super.to_s)
  end
end
puts Child.new.find([1, 3, 4, 5])
puts Child.new.find([1, 3, 5, 7])
