# Range no-block iteration/transform/filter → Enumerator (CRuby `enum.c`),
# extending the Array/Hash no-block wiring. The block form (direct or via
# the Range Enumerable fallback) handles the bounds when driven. Works for
# Int- and String-bounded finite ranges.
p (1..3).each.class                                  # Enumerator
p (1..5).each.to_a                                   # [1,2,3,4,5]
p (1..3).map.with_index { |x, i| [x, i] }            # [[1,0],[2,1],[3,2]]
p (1..3).collect.with_index { |x, i| x * 10 + i }    # [10,21,32]
p (1..5).select.with_index { |x, i| i.even? }        # [1,3,5]
p (1..5).reject.with_index { |x, i| i.even? }        # [2,4]
p (1..5).find.with_index { |x, i| i == 2 }           # 3
p (1..5).detect.with_index { |x, i| x == 4 }         # 4
p (1..3).each_with_index.to_a                         # [[1,0],[2,1],[3,2]]
p (1..4).partition.with_index { |x, i| i.even? }     # [[1,3],[2,4]]
p (1..3).group_by.with_index { |x, i| i.even? }      # {true=>[1,3],false=>[2]}
p (1..5).min_by.with_index { |x, i| -x }             # 5
p (1..5).max_by.with_index { |x, i| -x }             # 1
p (1..5).sort_by.with_index { |x, i| -x }            # [5,4,3,2,1]
p (1..3).each.next                                   # 1
p ('a'..'c').each.to_a                               # ["a","b","c"]
