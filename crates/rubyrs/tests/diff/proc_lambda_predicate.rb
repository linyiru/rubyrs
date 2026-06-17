# Proc#lambda? — the introspection bit. `->`, lambda{}, and
# Method/Symbol#to_proc are lambdas; proc{}/blocks are not. Compose
# follows the FIRST-EXECUTED function; curry preserves the flag.
# (rubyrs does NOT enforce lambda strict-arity/return — bit only.)
p (->(x) { x }).lambda?
p (-> {}).lambda?
p lambda {}.lambda?
p proc {}.lambda?
p Proc.new {}.lambda?

add = ->(a, b) { a + b }
p add.lambda?
inc = proc { |x| x + 1 }
p inc.lambda?

p method(:puts).to_proc.lambda?
p :upcase.to_proc.lambda?
p :upcase.to_proc.call("hi")

# Composition: lambda? of the first-executed (inner) function.
p (->(x) { x } >> ->(x) { x }).lambda?
p (proc { |x| x } >> proc { |x| x }).lambda?
p (proc { |x| x } << ->(x) { x }).lambda?
p (->(x) { x } << proc { |x| x }).lambda?

# Curry preserves the flag.
p add.curry.lambda?
p inc.curry.lambda?

# Block params carry their origin's flag.
def takes_block(&b)
  b.lambda?
end
p takes_block { 1 }
p takes_block(&-> { 1 })

# respond_to? agrees.
p proc {}.respond_to?(:lambda?)
