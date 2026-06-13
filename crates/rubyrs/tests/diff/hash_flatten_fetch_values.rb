# Hash#flatten / Hash#fetch_values / Hash#compare_by_identity? — native
# methods rack's Rack::Headers relies on (it supers into Hash#flatten
# and #fetch_values with downcased keys, and exposes
# compare_by_identity? for parity).

h = {"a" => 1, "b" => 2, "c" => 3}

# compare_by_identity? — always false (rubyrs never sets the bit).
p h.compare_by_identity?
p({}.compare_by_identity?)

# flatten — to_a.flatten(level); default level 1 spreads the pairs.
p h.flatten                       # ["a", 1, "b", 2, "c", 3]
p({}.flatten)                     # []
# Array VALUES stay nested at level 1, peel at level 2.
g = {x: [1, 2], y: 3}
p g.flatten                       # [:x, [1, 2], :y, 3]
p g.flatten(1)                    # [:x, [1, 2], :y, 3]
p g.flatten(2)                    # [:x, 1, 2, :y, 3]
# Negative level → full flatten.
p({a: [1, [2, [3]]]}.flatten(-1)) # [:a, 1, 2, 3]

# fetch_values — values in key order; KeyError on a missing key.
p h.fetch_values("a", "c")        # [1, 3]
p h.fetch_values                  # []
begin
  h.fetch_values("a", "zzz")
rescue KeyError => e
  puts "KeyError: #{e.message}"
end
