# Nested / parenthesized multiple-assignment targets — `(a, b) = ...`,
# `(a, b), c = ...`, `a, (b, c) = ...`, including nested splats and deep
# nesting. Surfaced by parser/current's generated lexer.
(a, b) = [1, 2]
p [a, b]                          # [1, 2]

(c, d), e = [1, 2], 3
p [c, d, e]                       # [1, 2, 3]

f, (g, h) = 1, [2, 3]
p [f, g, h]                       # [1, 2, 3]

# nested splat
(p1, *p2), p3 = [1, 2, 3], 4
p [p1, p2, p3]                    # [1, [2, 3], 4]

i, (j, *k) = :a, [:b, :c, :d]
p [i, j, k]                       # [:a, :b, [:c, :d]]

# deep nesting
((deep1, deep2),), = [[5, 6]]
p [deep1, deep2]                  # [5, 6]

# extra/missing elements fill nil / drop, like flat massign
(m, n), o = [1], 2, 3
p [m, n, o]                       # [1, nil, 2]

# nested target with a trailing splat at the outer level too
(q, r), *s = [10, 20], 30, 40
p [q, r, s]                       # [10, 20, [30, 40]]

# ivar nested targets
class Holder
  def set; (@x, @y), @z = [1, 2], 3; [@x, @y, @z]; end
end
p Holder.new.set                  # [1, 2, 3]

# the massign expression value is the original RHS
v = ((aa, bb) = [7, 8])
p v                               # [7, 8]
p [aa, bb]                        # [7, 8]
