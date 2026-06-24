# Since Ruby 3.2 Array#/Hash#/Range#to_set work WITHOUT an explicit
# `require "set"` — the first call triggers the Set autoload. Gems rely
# on this at load time (ActiveSupport delegation.rb: `%w(...).to_set`).
# A subsequent `require "set"` then returns false (already loaded).
p [1, 2, 2].to_set.size        # 2
p({ a: 1, b: 2 }.to_set.size)  # 2
p((1..3).to_set.sort)          # [1, 2, 3]
p [3, 1, 2].to_set.include?(2) # true
p require("set")               # false (autoload already loaded it)
