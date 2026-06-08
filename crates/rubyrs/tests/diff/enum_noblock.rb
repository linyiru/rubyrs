# A blockless native iterator returns an Enumerator (CRuby `enum.c`)
# that re-invokes `recv.meth(&block)` once driven. rubyrs now models a
# real Enumerator (preamble + Kernel#enum_for), so the no-block forms
# build one via `Vm::make_enum_for` instead of raising NoMethodError.

# --- Array ---
p [10, 20, 30].each.class                          # Enumerator
p [10, 20, 30].each.to_a                           # [10, 20, 30]
p [10, 20, 30].each.map { |x| x * 2 }              # [20, 40, 60]
p [1, 2, 3, 4].each.select(&:even?)                # [2, 4]
p [10, 20, 30].each_with_index.to_a                # [[10,0],[20,1],[30,2]]
p [10, 20, 30].each_with_index.map { |x, i| x + i } # [10, 21, 32]
p %w[a b c].each_index.to_a                         # [0, 1, 2]
p [1, 2, 3].each.with_index(1).to_a                # [[1,1],[2,2],[3,3]]
p [1, 2, 3, 4].each.with_object([]) { |x, a| a << x * x } # [1,4,9,16]
p [5, 6, 7].each.count                             # 3
p [5, 6, 7].each.first(2)                          # [5, 6]
p [5, 6, 7].each.include?(6)                        # true

# --- Hash ---
h = {a: 1, b: 2, c: 3}
p h.each.to_a                                       # [[:a,1],[:b,2],[:c,3]]
p h.each_pair.to_a                                  # [[:a,1],[:b,2],[:c,3]]
p h.each.map { |k, v| "#{k}:#{v}" }                 # ["a:1","b:2","c:3"]
p h.each_with_index.to_a                            # [[[:a,1],0],...]
p h.each_with_index.map { |pair, i| [pair, i] }     # same shape
p h.each_pair.select { |k, v| v > 1 }              # [[:b,2],[:c,3]]
