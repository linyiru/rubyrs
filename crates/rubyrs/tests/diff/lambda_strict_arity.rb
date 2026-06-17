# Lambdas enforce STRICT arity and do NOT auto-splat a single Array
# (a proc would). Wrong arg counts raise ArgumentError; procs stay
# lenient.
def t
  yield
rescue => e
  "#{e.class}: #{e.message}"
end

p t { ->(a, b) {}.call(1) }
p t { ->(a, b) {}.call(1, 2, 3) }
p t { ->(a, b = 1) {}.call }
p t { ->(a, b = 1) {}.call(1, 2, 3) }
p t { ->(a, *b) {}.call }
p t { ->(a) {}.call(1, 2) }
p t { ->() {}.call(1) }
# Lambdas do NOT auto-splat a single Array.
p t { ->(a, b) {}.call([1, 2]) }
# ...so a 2-arg lambda over array elements raises in map (no auto-splat).
p t { [[1, 2], [3, 4]].map(&->(a, b) { a + b }) }

# Correct-arity lambda calls succeed.
p ->(a, b) { a + b }.call(1, 2)
p ->(a, b = 10) { a + b }.call(1)
p ->(a, b = 10) { a + b }.call(1, 2)
p ->(a, *r) { [a, r] }.call(1, 2, 3)
p [[1, 2], [3, 4]].map(&->(pair) { pair.sum })

# Procs stay LENIENT (control): nil-fill, drop extra, auto-splat.
p proc { |a, b| [a, b] }.call(1)
p proc { |a, b| [a, b] }.call(1, 2, 3)
p proc { |a, b| [a, b] }.call([1, 2])
