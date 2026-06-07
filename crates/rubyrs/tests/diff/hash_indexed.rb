# Hash backed by an O(1) key index (insertion-ordered, mixed key
# types, default value/block, dup independence). The index must be
# transparent — semantics identical to CRuby's Hash.
h = {a: 1, b: 2, c: 3}
p h.to_a                       # insertion order
h[:b] = 20; p h.to_a           # update keeps position
h[:d] = 4;  p h.keys           # new key appended
h.delete(:a); p h.keys         # delete preserves order
p({ x: 1 }.merge({ y: 2, x: 9 }))
p h.each_with_object([]) { |(k, v), acc| acc << "#{k}=#{v}" }
g = Hash.new(0); "aab".chars.each { |c| g[c] += 1 }; p g
gd = Hash.new { |hh, k| hh[k] = [] }; gd[:z] << 1; gd[:z] << 2; p gd
d = h.dup; d[:new] = 99; p [h.key?(:new), d.key?(:new)]
p h.select { |k, v| v > 3 }
p h.fetch(:c)
p h.fetch(:missing, :dflt)
# mixed key types stay distinct
m = { 1 => :a, 1.0 => :b, "1" => :c, nil => :d, [1, 2] => :e }
p [m[1], m[1.0], m["1"], m[nil], m[[1, 2]]]
p m.keys.length
# build a larger hash then probe (exercises the index past small-n)
big = {}; 300.times { |i| big["k#{i}"] = i }
p [big.size, big["k150"], big["k299"], big.key?("k0"), big.key?("nope")]
