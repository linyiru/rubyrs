# Proc#arity with keyword params. A required keyword adds ONE to the
# count (regardless of how many); the sign rules differ proc vs lambda.
# Lambda — negative for rest / optional positional / kwrest / an
# optional keyword with no required keyword to anchor it.
p ->(a, b:){}.arity
p ->(a, b:, c:){}.arity
p ->(a, b: 1){}.arity
p ->(a, b:, c: 1){}.arity
p ->(a, **o){}.arity
p ->(b:){}.arity
p ->(b: 1){}.arity
p ->(a, b = 1, c:){}.arity
p ->(a, *r, c:){}.arity
p ->(a, b: 1, **o){}.arity
p ->(**o){}.arity
p ->(b:, c: 1){}.arity

# Proc — negative ONLY for a positional rest; keywords/optionals stay
# positive.
p proc { |a, b:| }.arity
p proc { |a, b: 1| }.arity
p proc { |a, **o| }.arity
p proc { |b:| }.arity
p proc { |a, b = 1, c:| }.arity
p proc { |a, b: 1, c:| }.arity
p proc { |a, *r, c:| }.arity
