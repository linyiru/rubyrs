# Parallel assignment coerces a non-Array RHS like CRuby (nil/scalar
# wrap to one element; to_ary honoured), so `a, b = nil` is [nil, nil],
# not a `nil[0]` NoMethodError.
a, b = nil; p [a, b]
c, d = 5; p [c, d]
e, f = [10, 20]; p [e, f]
g, h = [100]; p [g, h]
def two; [1, 2]; end
def scalar; 42; end
i, j = two; p [i, j]
k, l = scalar; p [k, l]
m, *n = [1, 2, 3, 4]; p [m, n]
x, y, z = [9]; p [x, y, z]
class Pair; def to_ary; [:a, :b]; end; end
q, r = Pair.new; p [q, r]
