# `Integer#chr` — Tier 1 byte ↔ String primitive. Returns a
# 1-byte binary String for the input in `0..255`; raises
# RangeError outside.
#
# CRuby's `chr(Encoding)` form (Unicode codepoints up to
# U+10FFFF) depends on a full encoding model that ADR 0017
# explicitly puts at Tier 3. The 0..255 byte form is what
# msgpack / pack-style binary protocols actually reach for and
# is the one this fixture pins.

# ASCII path — visible char comes back unchanged.
puts 65.chr                          # "A"
puts 97.chr                          # "a"
puts 48.chr                          # "0"

# Boundary values.
puts 0.chr.bytesize                  # 1
puts 127.chr.bytesize                # 1
puts 128.chr.bytesize                # 1
puts 255.chr.bytesize                # 1

# Round-trip — `chr` followed by `.bytes.first` reconstructs the
# original integer for every byte in 0..255.
all_match = (0..255).all? { |n| n.chr.bytes.first == n }
puts all_match                       # true

# Round-trip via pack/unpack — `chr` produces the same single-
# byte string that `pack("C")` would.
same = (0..255).all? { |n| n.chr == [n].pack("C") }
puts same                            # true

# Out-of-range raises RangeError with the expected message
# shape.
begin
  256.chr
rescue RangeError => e
  puts "RangeError: #{e.message}"    # "RangeError: 256 out of char range"
end

begin
  (-1).chr
rescue RangeError => e
  puts "RangeError: #{e.message}"    # "RangeError: -1 out of char range"
end

# `rescue StandardError` catches RangeError (RangeError <
# StandardError in the hierarchy).
begin
  9999.chr
rescue StandardError => e
  puts "via-StandardError: #{e.class.name}: #{e.message}"
end

# In iteration — common shape in binary-builder code (the
# pattern we hit while implementing Random#bytes).
buf = ""
[72, 101, 108, 108, 111].each { |b| buf << b.chr }
puts buf                             # "Hello"
puts buf.bytesize                    # 5
