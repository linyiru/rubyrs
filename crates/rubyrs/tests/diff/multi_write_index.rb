# `obj[idx, ...] = ...` on the LHS of a multi-write. The
# `MultiWriteTarget::Index` arm is the symmetric companion to
# `MWT::Call` (`obj.attr = ...`) — both let multi-write LHS
# expressions reach setter dispatch through an arbitrary
# receiver. Common shapes covered:
#
#   * Single-arg Array  index: `a[0], a[1] = x, y`
#   * Single-arg Hash   index: `h[:k], h[:m] = x, y`
#   * Mixed with Locals: `b, a[0] = x, y`
#   * Splat at the end:   `a[0], *rest = ...`
#   * Two-arg Array slice form: `a[1, 2] = [...]`

# Array.
a = [1, 2, 3]
a[0], a[1] = "x", "y"
p a

# Hash.
h = {a: 1, b: 2}
h[:a], h[:b] = "X", "Y"
p h

# Mix Index with Local on the LHS.
a2 = [10, 20]
b, a2[0] = 99, 88
puts b
p a2

# Splat tail with leading Index target.
arr = [1, 2, 3]
arr[0], *rest = 100, 200, 300
p arr
p rest

# Two-arg Array slice form (`a[start, length] = replacement`).
a3 = [1, 2, 3, 4, 5]
a3[1, 2], a3[3, 1] = ["X", "Y"], ["Z"]
p a3

# Index target with computed receiver — the receiver expression
# evaluates once per target, AFTER the RHS is computed (a
# documented Tier-1 divergence from CRuby's left-to-right rule
# but byte-identical on the common no-side-effects case).
data = {arr: [10, 20, 30]}
data[:arr][0], data[:arr][2] = "first", "last"
p data

# Index inside a method body — verifies the compiler's
# emit_store path works in non-toplevel scope.
def fill(a)
  a[0], a[1], a[2] = "a", "b", "c"
end
xs = [0, 0, 0, 0]
fill(xs)
p xs
