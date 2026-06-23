# Array#push / #append are variadic (only #<< is single-element).
a = [1]
p a.push(2, 3)
p a.push(*[4, 5])
p a.push
b = []
p b.append(:x, :y, :z)
c = [1]
c << 2
p c
