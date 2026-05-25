# Frozen strings — String#freeze / #frozen? / #dup + FrozenError
# on any mutating method (`<<`, `concat`, `prepend`, `replace`,
# `[]=`) against a frozen receiver.

s = "hello"
p s.frozen?

# freeze returns self.
ret = s.freeze
p s.frozen?
p ret.equal?(s)

# Mutating a frozen string raises FrozenError.
begin
  s << " world"
rescue FrozenError => e
  puts "caught <<: #{e.class.name}"
end

begin
  s.concat("!")
rescue FrozenError => e
  puts "caught concat: #{e.class.name}"
end

begin
  s.prepend("PRE-")
rescue FrozenError => e
  puts "caught prepend: #{e.class.name}"
end

begin
  s.replace("new")
rescue FrozenError => e
  puts "caught replace: #{e.class.name}"
end

begin
  s[0] = "H"
rescue FrozenError => e
  puts "caught []=: #{e.class.name}"
end

# String stays unchanged after every blocked mutation.
p s

# dup makes a fresh, non-frozen copy.
d = s.dup
p d.frozen?
d << " world"
p d
p s          # original untouched

# Non-mutating methods on a frozen string still work.
p s.upcase
p s.reverse
p s.length
p s.chars

# Re-freezing is a no-op (frozen stays true, no raise).
s.freeze
p s.frozen?

# Freeze through aliasing — both names share frozenness.
a = "shared"
b = a
a.freeze
p b.frozen?     # true — same Rc<RStr>

# But dup breaks the aliasing.
d2 = a.dup
p d2.frozen?    # false

# Frozen flag flows through method calls.
def hello
  s = "hi"
  s.freeze
  s
end

x = hello
p x.frozen?
begin
  x << "!"
rescue FrozenError
  puts "caught: method-returned frozen"
end

# StandardError rescues FrozenError (since FrozenError <
# RuntimeError < StandardError).
begin
  fresh = "x".freeze
  fresh << "y"
rescue StandardError => e
  puts "via StandardError: #{e.class.name}"
end

# `dup` of a frozen string equals it by content but isn't
# the same Rc.
orig = "fixed".freeze
copy = orig.dup
p orig == copy
p orig.equal?(copy)
