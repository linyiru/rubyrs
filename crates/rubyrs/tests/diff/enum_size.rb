# Enumerator#size — declared size for the generator form (the leading arg
# to Enumerator.new), else nil; the enum_for form counts via to_a (exact
# for the finite collections rubyrs enumerates, matching CRuby's result).
p [1, 2, 3].each.size                     # 3
p [1, 2, 3].map.size                      # 3
p [1, 2, 3].select.size                   # 3 (CRuby: source size)
p [1, 2, 3].reject.size                   # 3
p [].each.size                            # 0
p({a: 1, b: 2, c: 3}.each.size)           # 3
p({a: 1}.map.size)                        # 1
p (1..10).each.size                       # 10
p (1..10).map.size                        # 10
p [1, 2, 3, 4, 5].each_slice(2).size      # 3
p [1, 2, 3, 4].each_cons(2).size          # 3
p Enumerator.new { |y| y << 1; y << 2 }.size   # nil (no declared size)
p Enumerator.new(7) { |y| y << 1 }.size        # 7
p [10, 20, 30].each_with_index.size       # 3
