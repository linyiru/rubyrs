# Pure Float-key sum-by-key (ADR 0034 layer 3f): the JIT compiles
# `arr.each_with_object(Hash.new(0)) { |x, h| h[float_key] += int }` with a
# FLOAT bucket key (matched by the eql? Float arm: NaN by bits, else `==`, so
# -0.0 and 0.0 share a bucket). Parity must hold interpreter == JIT == CRuby.

# Float element used directly as key (count-by-key).
a = [1.5, 2.5, 1.5, 3.0, 2.5, 1.5]
p a.each_with_object(Hash.new(0)) { |x, h| h[x] += 1 }

# Float key, non-1 Int increment (sum-by-key).
b = [1.5, 2.5, 1.5, 3.0]
p b.each_with_object(Hash.new(0)) { |x, h| h[x] += 10 }

# Int element promoted to a Float key.
c = [1, 2, 1, 3, 2, 1]
p c.each_with_object(Hash.new(0)) { |x, h| h[x * 1.5] += 1 }

# -0.0 and 0.0 must collapse into one bucket (CRuby Hash-key semantics).
d = [0.0, -0.0, 0.0]
h = d.each_with_object(Hash.new(0)) { |x, k| k[x] += 1 }
p h
p h.keys.map(&:to_s)

# Distinct NaNs never collide — each its own bucket.
nan = 0.0 / 0.0
p [nan, nan, 1.0].each_with_object(Hash.new(0)) { |x, k| k[x] += 1 }.size

# Empty input → empty Hash.
p [].each_with_object(Hash.new(0)) { |x, k| k[x.to_f] += 1 }

# Larger, first-appearance key order preserved.
f = (0...30).map { |i| (i % 5).to_f + 0.5 }
p f.each_with_object(Hash.new(0)) { |x, k| k[x] += 1 }
