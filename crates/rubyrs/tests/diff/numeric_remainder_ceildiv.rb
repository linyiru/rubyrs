# Integer/Float #remainder (truncated, sign of dividend), Integer
# #ceildiv (Ruby 3.2+), and Float#div (floored quotient as Integer).
p (-7).remainder(3)
p 7.remainder(-3)
p 7.remainder(3)
p 0.remainder(5)
p 10.ceildiv(3)
p 9.ceildiv(3)
p (-10).ceildiv(3)
p 10.ceildiv(-3)
p 1.ceildiv(1)
p 5.0.div(2)
p 7.0.div(3)
p (-7.0).div(2)
p 5.0.div(2.5)
p 7.0.remainder(3)
p (-7.0).remainder(3)
p 7.remainder(2.0)

def t
  yield
rescue => e
  e.class
end
p t { 5.remainder(0) }
p t { 5.ceildiv(0) }
p t { 5.0.div(0) }

# Contrast: modulo keeps the divisor's sign.
p (-7) % 3
p (-7).modulo(3)
