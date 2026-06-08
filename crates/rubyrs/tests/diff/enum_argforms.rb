# Arg-bearing no-block forms still return an Enumerator carrying the arg
# (CRuby `enum.c`): min_by(n)/max_by(n) and each_with_object(memo). The
# arg is threaded into the Enumerator and re-applied when driven.
p [5, 3, 1, 4, 2].min_by(2).class                       # Enumerator
p [5, 3, 1, 4, 2].min_by(2).each { |x| x }              # [1, 2]
p [5, 3, 1, 4, 2].max_by(2).each { |x| x }              # [5, 4]
p [5, 3, 1, 4, 2].min_by(2).each { |x| -x }             # [5, 4]
p [1, 2, 3].each_with_object([]).class                  # Enumerator
p [1, 2, 3].each_with_object([]).each { |x, m| m << x * 2 }     # [2,4,6]
p [1, 2, 3].each_with_object({}).each { |x, m| m[x] = x * x }   # {1=>1,2=>4,3=>9}
p [1, 2, 3].each_with_object([]).with_index { |(x, m), i| m << [x, i] } # [[1,0],[2,1],[3,2]]
# Hash#each_with_object (no min_by(n)/max_by(n) block form on Hash)
h = {a: 1, b: 2}
p h.each_with_object([]).class                          # Enumerator
p h.each_with_object([]).each { |(k, v), m| m << "#{k}=#{v}" } # ["a=1","b=2"]
