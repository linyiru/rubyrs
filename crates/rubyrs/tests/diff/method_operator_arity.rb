# Method#arity for primitive-backed binary operators is 1 (was the
# generic -1 fallback); non-operator builtins keep -1.
p 5.method(:+).arity
p 5.method(:-).arity
p 5.method(:*).arity
p 5.method(:**).arity
p 5.method(:<=>).arity
p 5.method(:==).arity
p "a".method(:+).arity
p [].method(:<<).arity
p 5.method(:<).arity
p 5.method(:eql?).arity
