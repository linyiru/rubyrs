# Block auto-splat — when a block declared with more than one
# parameter is invoked with a single Array argument, the Array's
# elements are spread into the parameter slots. The canonical use
# cases:
#   arr_of_pairs.each { |a, b| ... }           # [[1,2],[3,4]]
#   hash.to_a.sort_by { |k, v| v }             # pair after to_a
# Hash#each / #map already yield two args directly, so this
# doesn't change their behaviour.

# Array-of-pairs iteration.
pairs = [[1, "a"], [2, "b"], [3, "c"]]
pairs.each do |k, v|
  puts "#{k}:#{v}"
end

# Single-param block — Array stays intact.
pairs.each do |item|
  p item
end

# Three-param block on a 2-elem Array — extras fall to nil.
[[1, 2], [3, 4]].each do |a, b, c|
  puts "#{a}/#{b}/#{c.inspect}"
end

# Hash#each — already yields (k, v) as two args; behavior unchanged.
h = {a: 1, b: 2, c: 3}
h.each do |k, v|
  puts "#{k}=#{v}"
end

# Hash#to_a → [[k, v], ...] then each — auto-splat fires.
h.to_a.each do |k, v|
  puts "to_a: #{k}=#{v}"
end

# each_with_index after to_a — block gets (pair, idx); pair stays
# an Array since block has 2 params and we yield (pair, idx).
h.to_a.each_with_index do |pair, i|
  puts "#{i}: #{pair.inspect}"
end

# But pull (k, v, i) from an inner Array via explicit indexing
# inside the block — common when you want both index + components.
h.to_a.each_with_index do |pair, i|
  puts "#{i}: #{pair[0]}=#{pair[1]}"
end

# sort_by chain.
sorted = {a: 3, b: 1, c: 2}.to_a.sort_by { |k, v| v }
sorted.each do |k, v|
  puts "sorted: #{k}=#{v}"
end

# map producing transformed pairs.
doubled = pairs.map do |k, v|
  [k * 2, v.upcase]
end
p doubled

# select on Array-of-Arrays.
keep = [[1, 2], [3, 4], [5, 6]].select { |a, b| a + b > 5 }
p keep

# reject on Array-of-Arrays.
drop = [[1, 2], [3, 4], [5, 6]].reject { |a, b| a + b > 5 }
p drop

# each_with_object with Array elements.
result = pairs.each_with_object({}) do |pair, memo|
  k, v = pair
  memo[k] = v
end
p result

# inject — block has (memo, elem); single-arg-becomes-pair doesn't
# apply (memo isn't an Array we want to splat). Confirm normal
# behavior.
total = [[1, 2], [3, 4]].inject(0) do |sum, pair|
  sum + pair[0] + pair[1]
end
puts total

# Three-element rows.
rows = [[1, 2, 3], [4, 5, 6], [7, 8, 9]]
rows.each do |a, b, c|
  puts "#{a} #{b} #{c}"
end

# Real-world idiom: zip then iterate.
xs = [1, 2, 3]
ys = ["a", "b", "c"]
xs.zip(ys).each do |x, y|
  puts "z #{x}#{y}"
end

# Nested arrays of varying depth — only the outer auto-splats.
[[1, [2, 3]], [4, [5, 6]]].each do |a, b|
  puts "#{a}: #{b.inspect}"
end
