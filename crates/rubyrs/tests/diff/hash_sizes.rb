# Hash semantics are identical across the key-index threshold (small
# hashes use a linear scan, large ones the O(1) key index): build,
# lookup, update, delete, key?, size, and insertion order all match.
[2, 15, 16, 17, 50].each do |n|
  h = {}
  (1..n).each { |i| h[i.to_s] = i * 10 }
  h["1"] = 999                 # update existing (keeps position)
  del = h.delete("2")          # delete
  p [n, h["1"], h[n.to_s], h["x"], h.key?("1"), h.key?("2"), del, h.size, h.keys.last]
end

# Integer keys across the boundary.
h2 = {}
(1..20).each { |i| h2[i] = i }
p [h2[10], h2.delete(10), h2[10], h2.size]

# Symbol keys across the boundary.
h3 = {}
%i[a b c d e f g h i j k l m n o p q r].each_with_index { |s, i| h3[s] = i }
p [h3[:a], h3[:r], h3.delete(:a), h3.key?(:a), h3.size]
