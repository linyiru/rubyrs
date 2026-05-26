# `Hash#each_key { |k| ... }` and `Hash#each_value { |v| ... }`
# — narrower siblings of `each` that yield only the key or
# only the value of each pair. Set#each had to detour through
# `.keys.each` before this commit; it's now the natural shape.
#
# CRuby's no-block form returns an Enumerator; Tier 1 doesn't
# model Enumerator (ADR 0017 row "Fiber / Enumerator" is
# Tier 2), so block-less calls fall through to NoMethodError.
# Fixture sticks to the block-given form.

# Basic collection — keys in insertion order, values likewise.
h = {a: 1, b: 2, c: 3}
keys = []
h.each_key { |k| keys << k }
p keys                              # [:a, :b, :c]

vals = []
h.each_value { |v| vals << v }
p vals                              # [1, 2, 3]

# Return value is the receiver Hash (matches `Hash#each`).
puts h.each_key { } == h            # true
puts h.each_value { } == h          # true

# Empty hash → block never fires.
fired = false
{}.each_key { |_| fired = true }
puts fired                          # false
fired = false
{}.each_value { |_| fired = true }
puts fired                          # false

# Mixed value types.
m = {"name" => "alice", :age => 30, [1, 2] => "pair-key"}
collected_keys = []
m.each_key { |k| collected_keys << k.inspect }
puts collected_keys.join(",")       # "\"name\",:age,[1, 2]"

collected_vals = []
m.each_value { |v| collected_vals << v.inspect }
puts collected_vals.join(",")       # "\"alice\",30,\"pair-key\""

# `break` inside the block returns the break value (early-exit
# semantics shared with `each`).
result = h.each_key do |k|
  break :nope if k == :b
end
puts result                         # nope

result = h.each_value do |v|
  break :stop if v == 2
end
puts result                         # stop

# Block can carry side effects affecting outer state; iteration
# count matches Hash size for a no-mutation block.
count_k = 0
{x: 1, y: 2}.each_key { |_| count_k += 1 }
puts count_k                        # 2

count_v = 0
{x: 1, y: 2}.each_value { |_| count_v += 1 }
puts count_v                        # 2

# Idiomatic use — pull only the side you need without
# allocating the Array `.keys` / `.values` would create.
total = 0
{a: 10, b: 20, c: 30}.each_value { |v| total += v }
puts total                          # 60
