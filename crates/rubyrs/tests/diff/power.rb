# Exponentiation operator ** plus its op-assign **=, across the
# numeric coercion lattice (Int/Float).

# Int ** Int with positive exponent → Int.
p 2 ** 10
p 3 ** 5
p 1 ** 100
p 0 ** 5
p 0 ** 0           # CRuby: 1
p (-2) ** 3        # CRuby: -8 (odd exponent preserves sign)
p (-2) ** 4        # CRuby:  16
p 10 ** 18
p 5 ** 1
p 1 ** 0

# Float ** Float.
p 2.0 ** 3.0
p 1.5 ** 2.0
p 2.0 ** 0.5
p 9.0 ** 0.5
p 4.0 ** -1.0
p 1.0 ** 100.0
p 0.0 ** 0.0       # 1.0
p (-1.0) ** 2.0

# Mixed Int ** Float (Int promotes).
p 2 ** 0.5
p 9 ** 0.5
p 4 ** -1.0
p 2 ** 2.0

# Mixed Float ** Int.
p 1.5 ** 2
p 2.0 ** 10
p 0.5 ** 3
p 0.5 ** -2

# **= on local.
a = 2
a **= 3
p a
a **= 2
p a

# **= on Float local.
b = 1.5
b **= 2
p b

# **= on ivar.
class Pow
  def initialize(base)
    @x = base
  end
  def square!
    @x **= 2
  end
  def cube!
    @x **= 3
  end
  def x; @x; end
end
p1 = Pow.new(3)
p1.square!
p p1.x
p1.cube!
p p1.x

# **= on Array index.
arr = [2, 3, 4]
arr[0] **= 2
arr[1] **= 3
arr[2] **= 4
p arr

# **= on Hash index.
h = {a: 2, b: 3}
h[:a] **= 4
h[:b] **= 2
p h

# Precedence: ** binds tighter than unary minus per CRuby.
p (-3) ** 2        # 9
p -(3 ** 2)        # -9

# In an expression.
def hypot(a, b)
  (a ** 2 + b ** 2) ** 0.5
end
p hypot(3, 4)
p hypot(5, 12)

# In iteration.
squares = [1, 2, 3, 4, 5].map { |n| n ** 2 }
p squares

# In a block returning the powered key.
sorted_by_sq = [3, 1, 4, 1, 5].sort_by { |n| n ** 2 }
p sorted_by_sq
