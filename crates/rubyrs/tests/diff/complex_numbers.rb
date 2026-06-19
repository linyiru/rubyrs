# Complex — pure-Ruby complex-number type, Kernel#Complex, to_c on
# the numerics, imaginary literals, and the numeric coerce protocol
# letting a built-in numeric LHS interoperate.

# construction + accessors
c = Complex(3, 4)
p c
p c.real
p c.imaginary
p c.imag
p c.abs
p c.abs2
p c.conjugate

# to_c on the numerics
p 3.to_c
p 2.5.to_c
p nil.to_c
p Rational(1, 2).to_c

# arithmetic among Complex
p Complex(1, 2) + Complex(3, 4)
p Complex(1, 2) - Complex(3, 4)
p Complex(1, 2) * Complex(3, 4)
p Complex(1, 2) / Complex(3, 4)
p Complex(0, 1) ** 2
p(-Complex(1, 2))

# built-in numeric LHS reaches Complex#coerce
p 1 + Complex(0, 1)
p 2 * Complex(1, 1)
p 5 - Complex(2, 3)
p 1.0 + Complex(0, 1)

# mixed component types
p Complex(1.5, 2.5)
p Complex(6, 4) / 2

# equality
p Complex(2, 3) == Complex(2, 3)
p Complex(5, 0) == 5
p Complex(2, 0) == 2.0
p Complex(1, 2) == Complex(1, 3)

# imaginary literals
p 3i
p 2.5i
p 1 + 2i
p 2i * 3i

# Kernel#Complex passthrough + string
p Complex(Complex(1, 2))
p Complex("2+3i")
p Complex("5")

# to_s vs inspect
p Complex(3, 4).to_s
p Complex(3, -4).to_s
p Complex(1, -2)

# real-only conversions
p Complex(5, 0).to_i
p Complex(2, 0).to_f
p Complex(7, 0).real?

# class hierarchy
p Complex.ancestors.include?(Numeric)
p Complex(1, 2).is_a?(Numeric)
