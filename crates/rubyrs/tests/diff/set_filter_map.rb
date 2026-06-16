# `Set#filter_map` (Enumerable map+compact). Surfaced by bridgetown-core's
# `configure_component_paths` filtering a Set of load paths.
require "set"
s = Set[1, 2, 3, 4, 5]
p s.filter_map { |x| x * 10 if x.even? }
p s.filter_map { |x| x }
e = s.filter_map
p e.class
