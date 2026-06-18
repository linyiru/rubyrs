# Kernel#Array expands a Range to its elements (it has a native to_a
# that the method-table coercion path missed → it used to wrap as
# [range]).
p Array(1..3)
p Array(1...3)
p Array('a'..'c')
p Array(1..1)
p Array(1..3).map { |x| x * 2 }
# The splat forms desugar through Array(), so they expand too.
p [*1..3]
a, b, c = *(1..3)
p [a, b, c]
# Other Array() inputs unchanged.
p Array(nil)
p Array([1, 2])
p Array(5)
p Array({a: 1})
