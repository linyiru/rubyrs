# `Set[...]` class-method constructor (== `Set.new([...])`).
require "set"

p Set[1, 2, 3].to_a.sort
p Set[].to_a
p Set[1, 1, 2].size          # dedups
p Set["a", "b"].include?("a")
p Set[1, 2] == Set[2, 1]
p Set[1, 2, 3].subset?(Set[1, 2, 3, 4])
p Set[3, 1, 2].to_a.sort
