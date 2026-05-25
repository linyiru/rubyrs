# Cross-type `==` must return false, never raise.
# Same-type stays value-equality. Same-type Array / Hash also
# benefits — value-equal via the existing `ruby_eq` helper.

# String compared to non-String
puts("x" == nil)
puts("x" != nil)
puts("x" == 5)
puts("x" == :sym)
puts("x" == [])
puts("x" == {})

# Nil compared to anything
puts(nil == "x")
puts(nil == 0)
puts(nil == false)        # CRuby: false; nil and false are not the same Object
puts(nil == nil)
puts(nil != nil)

# Integer compared to non-Integer
puts(5 == "5")
puts(5 == :five)
puts(5 == nil)
# 5 == 5.0 would be true in CRuby (numeric coercion) — rubyrs
# doesn't have Float yet, so the literal would SyntaxError. Skip.

# Symbol compared to String
puts(:foo == "foo")
puts(:foo == :foo)

# Array value-equality
puts([1, 2] == [1, 2])
puts([1, 2] == [1, 3])
puts([] == [])
puts([1, "x"] == [1, "x"])

# Hash value-equality (keys + values, order-insensitive in CRuby;
# rubyrs uses position-sensitive `ruby_eq` — same content same
# insertion order matches; reordered is a documented divergence
# tracked in SUBSET.md). We only test same-order cases here.
puts({a: 1} == {a: 1})
puts({a: 1} == {a: 2})
puts({} == {})

# Range
puts((1..3) == (1..3))
puts((1..3) == (1..4))
puts((1..3) == (1...3))   # inclusive vs exclusive differ

# Cross-type chain — common idiom in guards
v = nil
if v == "ready"
  puts "matched"
else
  puts "not ready"
end
