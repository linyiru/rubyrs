# Integer() / Float() / String() — strict conversion functions.
# Unlike to_i / to_f (lenient — "abc".to_i is 0), these raise
# ArgumentError on input that can't be cleanly parsed. Canonical
# Ruby idiom: `port = Integer(ENV['PORT']) rescue 8080`.

# Integer — happy path.
p Integer(42)
p Integer("42")
p Integer("  -7  ")
p Integer("0")
p Integer(3.7)
p Integer(-3.7)
p Integer(0.0)

# Integer — argument errors raise ArgumentError.
begin
  Integer("abc")
rescue ArgumentError => e
  puts "ie1: #{e.message}"
end
begin
  Integer("42abc")
rescue ArgumentError => e
  puts "ie2: #{e.message}"
end
begin
  Integer("")
rescue ArgumentError => e
  puts "ie3: #{e.message}"
end

# Integer — nil raises TypeError.
begin
  Integer(nil)
rescue TypeError => e
  puts "te: #{e.message}"
end

# Float — happy path.
p Float(3.14)
p Float(42)
p Float("3.14")
p Float("  -2.5e3  ")
p Float("0")

# Float — argument error.
begin
  Float("not a number")
rescue ArgumentError => e
  puts "fe: #{e.message}"
end

# Float — nil errors.
begin
  Float(nil)
rescue TypeError => e
  puts "fnil: #{e.message}"
end

# String — calls to_s; never raises for our built-in types.
p String(42)
p String(3.14)
p String("hello")
p String(:foo)
p String(nil)         # CRuby: ""
p String(true)
p String(false)

# Used in the canonical "convert or default" idiom via inline
# rescue.
def safe_int(s)
  Integer(s) rescue -1
end

puts safe_int("42")
puts safe_int("0")
puts safe_int("nope")
puts safe_int(nil)
puts safe_int(3.7)

def safe_float(s)
  Float(s) rescue 0.0
end

puts safe_float("3.14")
puts safe_float("nope")

# Use in a method chain.
def parse_pair(s)
  parts = s.split(",")
  [Integer(parts[0]), Integer(parts[1])]
end

p parse_pair("3,7")
p parse_pair("100,-2")

# Coerce per-element from a CSV-ish input.
sums = ["1,2", "10,20", "100,200"].map do |row|
  parts = row.split(",")
  Integer(parts[0]) + Integer(parts[1])
end
p sums
