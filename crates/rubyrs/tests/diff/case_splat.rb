# Splat in case/when — `when *arr` matches if any of arr's elements
# === the predicate. Translates to `arr.any? { |x| x === predicate }`.

fruits = ["apple", "banana", "cherry"]
veggies = ["carrot", "celery"]

["apple", "carrot", "bread", "banana"].each do |item|
  category = case item
             when *fruits then "fruit"
             when *veggies then "veg"
             else "other"
             end
  puts "#{item}: #{category}"
end

# Splat alongside literal when-conditions on the same case.
def classify(n)
  small = [1, 2, 3]
  big = [100, 200]
  case n
  when *small then "small"
  when 50 then "medium"
  when *big then "big"
  else "unknown"
  end
end
puts classify(2)    # small
puts classify(50)   # medium
puts classify(200)  # big
puts classify(42)   # unknown

# Empty splat array — every match falls through.
empty = []
puts(case "x"; when *empty then "got it"; else "fallthrough"; end)
