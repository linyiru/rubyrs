# Proc#arity with optional positional params. CRuby distinguishes
# procs from lambdas: an optional makes a LAMBDA's arity negative
# (-(required+1)) but leaves a lenient PROC's arity positive
# (just the required count). A rest param is always negative.
p ->(a, b = 10) {}.arity
p ->(a, b = 2, c = 3) {}.arity
p ->(a, b = 9, *r) {}.arity
p ->(a, b) {}.arity
p ->(a, *b) {}.arity
p ->() {}.arity
p ->(a) {}.arity
p ->(*a) {}.arity
p ->(a, b = 1, c = 2, *r) {}.arity
p lambda { |a, b = 1| }.arity

# Procs: optional does NOT make arity negative; rest does.
p proc { |a, b = 5| }.arity
p proc { |a, b| }.arity
p proc { |a, *b| }.arity
p proc { |*a| }.arity
p proc { |a, b = 1, c = 2| }.arity
p proc { |a, b = 1, *r| }.arity

# Method/Symbol-to-proc lambdas carry their own arity shape.
p ->(a, b = 1) {}.lambda?
