# Hash#min_by(n) / #max_by(n) — the n smallest/largest [k,v] pairs by
# the block's value, as an Array of pairs.
h = {a: 1, b: 2, c: 3}
p h.min_by(2) { |k, v| v }
p h.max_by(2) { |k, v| v }
p h.min_by(1) { |k, v| v }
p h.min_by(0) { |k, v| v }
p h.min_by(5) { |k, v| v }
p({a: 3, b: 1, c: 2}.min_by(2) { |k, v| v }.to_h)
p({}.min_by(2) { |k, v| v })
# no-arg form unchanged
p h.min_by { |k, v| v }
p h.max_by { |k, v| v }
