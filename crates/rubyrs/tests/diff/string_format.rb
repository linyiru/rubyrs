# String#% — printf-style formatting.

# Basic substitutions.
puts "Hello, %s!" % "world"
puts "x=%d" % 42
puts "f=%f" % 3.14
puts "%s=%d" % ["age", 30]
puts "name: %s, score: %d" % ["Alice", 95]

# Width and right-alignment.
puts "[%5d]" % 3
puts "[%5d]" % 12345
puts "[%5s]" % "hi"

# Left-alignment.
puts "[%-5d]" % 3
puts "[%-5s|end]" % "hi"

# Zero-pad.
puts "[%05d]" % 3
puts "[%08d]" % 42

# Sign flags.
puts "[%+d]" % 7
puts "[%+d]" % -7
puts "[% d]" % 7
puts "[% d]" % -7

# Float precision.
puts "%.3f" % 3.14159
puts "%.0f" % 3.7
puts "%.6f" % 1.0
puts "%10.3f" % 3.14159
puts "%-10.3f|end" % 3.14159
puts "%+.2f" % 0.5
puts "%+.2f" % -0.5

# Integer with precision (zero-pad).
puts "%.4d" % 12
puts "%.4d" % 1234567

# Hex, octal, binary.
puts "%x" % 255
puts "%X" % 255
puts "%#x" % 255
puts "%#X" % 255
puts "%o" % 8
puts "%#o" % 8
puts "%b" % 10
puts "%08b" % 10
puts "%#b" % 10

# Literal %.
puts "100%%"
puts "%d%%" % 50

# Character.
puts "%c" % 65
puts "%c" % "Z"

# Inspect (%p).
puts "%p" % "hello"
puts "%p" % :foo
puts "%p" % 42
puts "%p" % nil

# Float coercion from Int for %f.
puts "%.2f" % 5

# Multiple specs in one format.
puts "(%d, %d, %d)" % [1, 2, 3]
puts "%s: %d items at $%.2f each" % ["widgets", 3, 9.95]

# %s on non-string coerces via to_s.
puts "result=%s" % 42
puts "flag=%s" % true
puts "n=%s" % nil

# Truncation via precision on %s.
puts "[%.3s]" % "abcdef"
puts "[%10.3s]" % "abcdef"
puts "[%-10.3s|end]" % "abcdef"

# rescue path: too-few arguments yields a script-catchable
# ArgumentError (the Trap-to-rescue route from a prior commit).
begin
  "%s %s" % ["only"]
rescue ArgumentError
  puts "rescued: too few args"
end

# Method-call style chains.
def greeting(name, n)
  "hi %s — %d msgs" % [name, n]
end
puts greeting("Bob", 7)

# Width and precision on the same %s.
[1, 22, 333].each do |n|
  puts "%5d: %.4f" % [n, n / 7.0]
end
