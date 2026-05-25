# Kernel#p / Kernel#pp — print each arg's inspect form, one per
# line. Return value: nil for 0 args, the arg for 1, the args
# Array for 2+.

# Basic types — each goes through `inspect`.
p 1
p 1.5
p "hello"
p :sym
p nil
p true
p false

# Arrays & Hashes use inspect.
p [1, 2, 3]
p [["nested", "arr"], 1]
p({"a" => 1, :b => 2})

# Range.
p (1..5)
p (1...5)

# Multiple args — one inspect-line per arg.
p 1, "two", :three

# Return value: 0 args → nil
result0 = p
puts result0.inspect

# Return value: 1 arg → that arg
result1 = p 42
puts result1.class.name

# Return value: 2+ args → Array of args
result2 = p 1, 2, 3
puts result2.inspect
puts result2.class.name

# Chains: `p` returns its arg, so it slots into expressions.
def double(x)
  x * 2
end
puts double(p 5)

# pp is an alias for p in this subset.
pp({a: 1, b: 2})
pp [1, 2, 3]
pp "test"

# p inside an iterator.
[1, 2, 3].each { |n| p n * n }

# p with a method-call result.
def info
  {name: "rubyrs", year: 2026}
end
p info