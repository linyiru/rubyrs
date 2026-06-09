# Array#flatten recurses FULLY by default (was depth-1 only); flatten(n)
# caps the depth; flatten! mutates in place and returns nil on no change.

p [1, [2, [3, [4]]]].flatten
p [[1, 2], [3, 4]].flatten
p [1, 2, 3].flatten
p [].flatten

# depth arg
p [1, [2, [3]]].flatten(1)
p [1, [2, [3]]].flatten(2)
p [1, [2, [3]]].flatten(0)        # 0 → unchanged
p [1, [2, [3, [4, [5]]]]].flatten(2)
p [1, [2, [3]]].flatten(nil)      # nil → unlimited

# mixed-content
p [1, "a", [2, :b, [3.0]]].flatten
p [[nil], [true, [false]]].flatten

# flatten! (in place)
a = [1, [2, [3]]]
p a.flatten!                      # [1, 2, 3]
p a                               # mutated
p [1, 2, 3].flatten!              # nil — nothing to flatten
b = [1, [2]]
b.flatten!(1)
p b

# recursive array → ArgumentError
begin
  r = [1, 2]
  r << r
  r.flatten
rescue ArgumentError => e
  p e.message
end
