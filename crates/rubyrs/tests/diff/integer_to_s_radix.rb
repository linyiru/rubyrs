# `Integer#to_s(radix)` — Tier 1 base conversion. Accepts radix
# 2..=36; lowercase digits for radix > 10 (`a..z`). CRuby's exact
# error message shape on out-of-range radix.
#
# Workarounds eliminated by this commit:
#   - SecureRandom UUID byte→hex formatting was using
#     `pack("C*").unpack("H*")` to bypass missing `to_s(16)`.
#   - msgpack BigInt wire-protocol scratch space.

# Common bases — hex / binary / octal.
puts 255.to_s(16)             # "ff"
puts 255.to_s(2)              # "11111111"
puts 8.to_s(8)                # "10"
puts 1000.to_s(16)            # "3e8"
puts 0xDEAD.to_s(16)          # "dead"

# Boundary radices.
puts 0.to_s(2)                # "0"
puts 0.to_s(36)               # "0"
puts 1.to_s(2)                # "1"
puts 35.to_s(36)              # "z" (last single-digit in base 36)
puts 36.to_s(36)              # "10"

# Negative receivers — leading "-" then magnitude in radix.
puts (-1).to_s(2)             # "-1"
puts (-15).to_s(16)           # "-f"
puts (-255).to_s(16)          # "-ff"

# Default-radix shape unchanged (no-arg `to_s`).
puts 255.to_s                 # "255"
puts 0.to_s                   # "0"
puts (-42).to_s               # "-42"

# i64::MAX / i64::MIN edges — using `unsigned_abs` ensures
# i64::MIN doesn't overflow on the sign-flip path.
puts 9223372036854775807.to_s(16)              # i64::MAX in hex
puts (-9223372036854775808).to_s(16)           # i64::MIN in hex

# Out-of-range radix raises ArgumentError with CRuby's exact
# message shape.
[0, 1, 37, -1, -16].each do |bad|
  begin
    5.to_s(bad)
    puts "no-raise for #{bad}"
  rescue ArgumentError => e
    puts "AE radix #{bad}: #{e.message}"
  end
end

# Spot-checked exact outputs for the 0..16 range in hex —
# the standard hex-nibble digits. (Round-trip via
# `String#to_i(radix)` would be the natural shape but the
# two-arg `to_i` form is a separate Tier 1 gap not covered
# here.)
hex_table = (0..16).map { |n| n.to_s(16) }.join(",")
puts hex_table                # "0,1,2,3,4,5,6,7,8,9,a,b,c,d,e,f,10"
