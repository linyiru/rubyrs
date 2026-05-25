# String#encode and #force_encoding — stub implementations.
# The subset stores raw bytes with no per-string encoding tag,
# so both are effectively no-ops (return the receiver). Useful
# for compatibility with code that defensively calls these
# during boundary handling. Cross-encoding conversion is
# explicitly out of scope (documented in SUBSET.md).

s = "hello"

# encode returns a String equal in content.
puts s.encode("UTF-8")                  # hello
puts s.encode("UTF-8").class.name       # String
puts s.encode("UTF-8") == s             # true

# force_encoding returns the receiver.
puts s.force_encoding("UTF-8").class.name  # String
puts s.force_encoding("UTF-8") == s        # true

# Stored and reused — call doesn't mutate.
s2 = "world"
s2.encode("UTF-8")
puts s2                                 # world (unchanged)

# Chain with other String ops.
puts "Hello".encode("UTF-8").upcase     # HELLO
puts "MIXED".force_encoding("UTF-8").downcase  # mixed

# Round-trips through a method body.
class Encoder
  def normalise(s); s.force_encoding("UTF-8").encode("UTF-8"); end
end
puts Encoder.new.normalise("abc")       # abc
