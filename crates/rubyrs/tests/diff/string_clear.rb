# String#clear — empty the buffer in place, return self (same object),
# keep encoding, raise FrozenError when frozen. Surfaced by net/protocol's
# rbuf_flush (`@rbuf.clear`).
s = "hello world"
r = s.clear
p s            # ""
p r.equal?(s)  # true — returns the same object
p s.empty?     # true
p s.length     # 0

# Mutation is visible through an alias (same object).
a = "data"
b = a
a.clear
p b            # ""

# Encoding is preserved (UTF-8 stays UTF-8).
u = "café"
u.clear
p u.encoding.name

# Frozen → FrozenError.
begin
  "frozen".freeze.clear
  puts "NO RAISE (wrong)"
rescue FrozenError
  puts "FrozenError"
end
