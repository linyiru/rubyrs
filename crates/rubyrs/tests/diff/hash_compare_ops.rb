# Hash comparison operators `< <= > >=` — subset/superset semantics
# (CRuby hash.c rb_hash_lt/le/gt/ge): `a <= b` iff every [key, value]
# pair of `a` is present in `b` (value compared with ==); `<` strict.
# Non-Hash operand goes through implicit to_hash conversion, TypeError
# otherwise. Motivating consumer: rubocop 1.88's
# Options#invalid_arguments_for_parallel compares the parsed flag hash
# with `>` on every multi-file (default) run.
small = { a: 1 }
big = { a: 1, b: 2 }

p small < big
p small <= big
p big > small
p big >= small
p big < small
p small > big

# Equal hashes: subset, but not a PROPER subset
same = { a: 1 }
p small < same
p small <= same
p small > same
p small >= same

# Same size, different value / different key — no subset either way
p({ a: 1 } < { a: 2 })
p({ a: 1 } <= { a: 2 })
p({ a: 1 } >= { b: 1 })

# The value must match with ==, not just the key
p({ a: 1 } <= { a: 1.0, b: 2 })
p({ a: "x" } <= { a: "y", b: 2 })

# Empty hash is a proper subset of anything non-empty, never of itself
p({} < { a: 1 })
p({} <= {})
p({} < {})

# Non-Symbol keys and structured values
p({ "k" => [1, 2] } <= { "k" => [1, 2], "j" => 3 })
p({ "k" => [1, 2] } <= { "k" => [1, 3], "j" => 3 })

# Operand with to_hash — implicit conversion (rb_to_hash_type)
class HashLike
  def to_hash
    { a: 1, b: 2 }
  end
end
p({ a: 1 } < HashLike.new)
p({ a: 1, b: 2, c: 3 } > HashLike.new)

# Non-Hash operand → TypeError with CRuby's message shape
class NotHashLike; end
[1, nil, true, [1], "s", :sym, NotHashLike.new].each do |bad|
  begin
    { a: 1 } < bad
  rescue TypeError => e
    puts e.message
  end
end

# Wrong arity → ArgumentError
begin
  { a: 1 }.send(:<)
rescue ArgumentError => e
  puts e.message
end
begin
  { a: 1 }.send(:>=, {}, {})
rescue ArgumentError => e
  puts e.message
end

# send-form dispatch (method-call path, not the operator opcode)
p({ a: 1 }.send(:<, { a: 1, b: 2 }))
p({ a: 1, b: 2 }.send(:>, { b: 2 }))

# respond_to?
p({}.respond_to?(:<))
p({}.respond_to?(:>=))
