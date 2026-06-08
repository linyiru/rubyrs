# each_slice(n) / each_cons(n) (no block) now return a real Enumerator
# (was a materialized Array stopgap). .class is Enumerator; .to_a keeps
# the same shape, and the full Enumerator surface (next/with_index/map/
# size) works. Block forms re-invoked via make_enum_for.
p [1, 2, 3, 4, 5].each_slice(2).class                # Enumerator
p [1, 2, 3, 4, 5].each_slice(2).to_a                 # [[1,2],[3,4],[5]]
p [1, 2, 3, 4, 5].each_slice(2).next                 # [1,2]
p [1, 2, 3, 4, 5].each_slice(2).map { |s| s.sum }    # [3,7,5]
p [1, 2, 3, 4, 5].each_slice(2).size                 # 3
p [1, 2, 3, 4, 5].each_slice(2.9).to_a               # float coerce -> [[1,2],[3,4],[5]]
p [1, 2, 3, 4].each_cons(2).class                    # Enumerator
p [1, 2, 3, 4].each_cons(2).to_a                      # [[1,2],[2,3],[3,4]]
p [1, 2, 3, 4].each_cons(2).map { |a, b| a + b }     # [3,5,7]
p [1, 2].each_cons(3).to_a                            # [] (window > len)
# Hash
h = {a: 1, b: 2, c: 3, d: 4, e: 5}
p h.each_slice(2).class                              # Enumerator
p h.each_slice(2).to_a                                # [[[:a,1],[:b,2]],[[:c,3],[:d,4]],[[:e,5]]]
p h.each_cons(2).to_a.length                          # 4
# eager arg validation (CRuby raises here, not on drive)
begin; [1].each_slice(0); rescue => e; puts "#{e.class}: #{e.message}"; end   # invalid slice size
begin; [1].each_cons(0); rescue => e; puts "#{e.class}: #{e.message}"; end    # invalid size
begin; [1].each_slice(-2); rescue => e; puts "#{e.class}: #{e.message}"; end  # invalid slice size
