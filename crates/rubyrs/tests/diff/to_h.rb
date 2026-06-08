# Array#to_h (no block): array of [k, v] pairs -> Hash, dedup keeps the
# first position with the last value. With a block: each element is
# mapped to a [k, v] pair. Enumerator#to_h mirrors it (Enumerable form,
# whose error wording differs from Array's — no index, "element has
# wrong array length").
p [[1, 2], [3, 4]].to_h                             # {1=>2, 3=>4}
p [[:a, 1], [:b, 2]].to_h                           # {a: 1, b: 2}
p [].to_h                                           # {}
p [[1, :a], [1, :b]].to_h                           # {1=>:b}  (last wins)
p [1, 2, 3].to_h { |x| [x, x * x] }                 # {1=>1, 2=>4, 3=>9}
p %w[a bb ccc].to_h { |s| [s, s.length] }           # {"a"=>1,"bb"=>2,"ccc"=>3}
# via Enumerator
p [1, 2, 3].each_with_index.to_h                    # {1=>0, 2=>1, 3=>2}
p({x: 1, y: 2}.each.to_h)                            # {x: 1, y: 2}
p [1, 2].each.to_h { |n| [n, n.to_s] }              # {1=>"1", 2=>"2"}
# error parity — Array#to_h (indexed wording)
begin; [1, 2].to_h; rescue => e; puts "#{e.class}: #{e.message}"; end
begin; [[1, 2, 3]].to_h; rescue => e; puts "#{e.class}: #{e.message}"; end
begin; [[1, 2], :x].to_h; rescue => e; puts "#{e.class}: #{e.message}"; end
# error parity — Enumerator#to_h (Enumerable wording, no index)
begin; [1, 2, 3].each.to_h; rescue => e; puts "#{e.class}: #{e.message}"; end
begin; [[1, 2, 3]].each.to_h; rescue => e; puts "#{e.class}: #{e.message}"; end
