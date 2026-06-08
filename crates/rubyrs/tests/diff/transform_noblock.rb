# The transform/filter Enumerable family returns an Enumerator when
# called without a block (CRuby `enum.c`); rubyrs builds one via
# make_enum_for, re-invoking the block form once driven. Exercised here
# through `.with_index { }` / `.to_a` / `.each { }`.
#
# Also guards Enumerator#with_index: its each-block must return the USER
# block's value (not the `i += 1` counter), or map/select/sort_by would
# collect/filter on the counter.

# --- Array ---
p [10, 20, 30].map.class                              # Enumerator
p [10, 20, 30].map.with_index { |x, i| [x, i] }       # [[10,0],[20,1],[30,2]]
p [1, 2, 3, 4].select.with_index { |x, i| i.even? }   # [1, 3]
p [1, 2, 3, 4].reject.with_index { |x, i| i.even? }   # [2, 4]
p [1, 2, 3].flat_map.with_index { |x, i| [x, i] }     # [1,0,2,1,3,2]
p [1, 2, 3, 4].filter_map.with_index { |x, i| x if i.even? } # [1,3]
p %w[a bb ccc].group_by.with_index { |s, i| i.even? } # {true=>["a","ccc"],false=>["bb"]}
p [3, 1, 2].min_by.with_index { |x, i| x }            # 1
p [3, 1, 2].max_by.with_index { |x, i| x }            # 3
p [3, 1, 2].sort_by.with_index { |x, i| x }           # [1,2,3]
p [1, 2, 3, 4].partition.with_index { |x, i| i.even? }# [[1,3],[2,4]]
p [10, 20, 30].find.with_index { |x, i| i == 1 }      # 20
p [10, 20, 30].detect.with_index { |x, i| x == 30 }   # 30
p [10, 20, 30].reverse_each.to_a                      # [30,20,10]
p [5, 6, 7].find_index.each { |x| x > 5 }             # 1
p [1, 2, 3].map.to_a                                  # [1,2,3]
p [1, 2, 3].collect.with_index { |x, i| x * 10 + i }  # [10,21,32]

# --- Hash ---
h = {a: 1, b: 2, c: 3}
p h.map.with_index { |(k, v), i| [k, v, i] }          # [[:a,1,0],[:b,2,1],[:c,3,2]]
p h.select.with_index { |(k, v), i| i.even? }         # {a:1,c:3}
p h.reject.with_index { |(k, v), i| i.even? }         # {b:2}
p h.find.with_index { |(k, v), i| i == 1 }            # [:b,2]
p h.sort_by.with_index { |(k, v), i| -v }             # [[:c,3],[:b,2],[:a,1]]
p h.flat_map.with_index { |(k, v), i| [k, i] }        # [:a,0,:b,1,:c,2]
