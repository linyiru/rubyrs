# `String.new` — Tier 1 primitive constructor.
#
# Pre-fix divergence: rubyrs's generic `Class.new` allocator
# returned a `Value::Object` (Instance with `class = String`)
# rather than a `Value::Str`. Every String primitive
# (`length`, `<<`, `bytesize`, `+`, …) dispatched on
# `Value::Str` and so `NoMethodError`'d on a generic-Object
# `String.new`. Now intercepted: 0-arg returns an empty
# `Value::Str`, 1-arg copies the given String, anything else
# raises TypeError / ArgumentError to match CRuby's shape.

# Identity + class.
s = String.new
puts s.class.name                    # "String"
puts s.is_a?(String)                 # true

# 0-arg shape — empty string with the usual String API.
puts s.length                        # 0
puts s.empty?                        # true
puts s.inspect                       # "\"\""

# Mutability — `<<` and `+` work because the receiver is a real
# `Value::Str` now.
s << "hello"
s << " " << "world"
puts s                               # "hello world"
puts s.length                        # 11
puts s + "!"                         # "hello world!"

# 1-arg shape — copy of the source String.
seeded = String.new("seeded")
puts seeded.class.name               # "String"
puts seeded                          # "seeded"
puts seeded.length                   # 6
# Mutating the copy doesn't change the source literal.
seeded << "++"
puts seeded                          # "seeded++"
puts "seeded".length                 # 6 (original unchanged)

# Build-a-buffer idiom — the canonical pattern this fix
# unblocks (Random#bytes / SecureRandom helpers used to need
# `Array#pack("C*")` workarounds because of this gap).
buf = String.new
[72, 101, 108, 108, 111].each { |b| buf << b.chr }
puts buf                             # "Hello"
puts buf.bytesize                    # 5

# Non-String positional arg → TypeError with CRuby's exact
# message shape.
begin
  String.new(42)
rescue TypeError => e
  puts "TypeError: #{e.message}"     # "no implicit conversion of Integer into String"
end

# 2+ positional args → ArgumentError.
begin
  String.new("a", "b")
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}" # "wrong number of arguments (given 2, expected 0..1)"
end
