# Float#to_s / #inspect match CRuby's dtoa: FIXED notation when the
# decimal point lands in -3..=15, SCIENTIFIC (D.DDDe±EE, signed ≥2-digit
# exponent, mantissa always with a fractional digit) otherwise.
[0.0, -0.0, 1.0, -1.0, 100.0, 0.1, 0.5, 3.14, -3.14,
 1e15, 1e16, 1e17, 1e20, 1.5e20, 9999999999999998.0,
 1e-3, 1e-4, 1e-5, 1e-7, 0.0001, 0.00001, 123456.789,
 1234567890123456.0, 12345678901234567.0, 1e100, 1e-100,
 2.5e-8, -3.7e22, 0.30000000000000004, 1.0/3.0, 6.022e23,
 (1.0/0.0), (-1.0/0.0), (0.0/0.0)].each { |f| puts f.to_s }
# inspect inside a collection uses the same renderer
p [1.0, 1e20, 1e-7, 0.5]
p({ big: 1e16, small: 2.5e-8 })
puts "interp #{1e20} and #{0.001}"
