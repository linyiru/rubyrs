# Kernel#Hash(arg): nil or [] → {}; a Hash → itself; an object with
# #to_hash → its result; anything else raises TypeError. (Narrower than
# Array() — it never wraps an arbitrary value.)
p Hash(nil)
p Hash([])
p Hash({a: 1, b: 2})

def t
  yield
rescue => e
  e.class
end
p t { Hash([[1, 2]]) }
p t { Hash(5) }
p t { Hash("x") }
p t { Hash(:sym) }

# to_hash coercion.
class WithToHash
  def to_hash; {converted: true}; end
end
p Hash(WithToHash.new)

# method(:Hash) capture round-trips through the explicit-recv path.
p method(:Hash).call(nil)
