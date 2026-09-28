# `Set#!=` negates `Set#==` (content equality). ActiveRecord's `insert_all`
# checks `keys_including_timestamps != attributes.keys.to_set` and raised
# "All objects being inserted must have the same keys" while this was true.
require "set"
p Set[1, 2] != Set[2, 1], Set[1] != Set[2]
p [1, 2].to_set + [3] != Set[1, 2, 3]
keys = %w[title body].to_set
p keys + %w[created_at updated_at] != %w[title body created_at updated_at].to_set
p Set[] != Set[], Set[1] != [1]
