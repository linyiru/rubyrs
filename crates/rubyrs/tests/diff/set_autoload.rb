# Set is an autoloaded core class since Ruby 3.2 — usable WITHOUT an explicit
# `require "set"`. Surfaced by the gem survey: multi_json does `Set.new` at
# load time assuming Set is core.
s = Set.new([1, 2, 2, 3])
p s.size                              # 3
p s.include?(2)                       # true
p s.include?(9)                       # false
p Set[1, 2, 3].to_a.sort              # [1, 2, 3]
p (Set[1, 2] | Set[2, 3]).to_a.sort   # [1, 2, 3]
p (Set[1, 2, 3] & Set[2, 3, 4]).to_a.sort # [2, 3]
p (Set[1, 2, 3] - Set[2]).to_a.sort   # [1, 3]
p Set[1, 2].subset?(Set[1, 2, 3])     # true

# An explicit require still works (idempotent with the autoload).
require "set"
p Set.new(%w[a b a]).size             # 2
