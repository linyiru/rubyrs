# `Kernel#Integer(str, radix)` — strict counterpart to
# `String#to_i(radix)`. Any garbage tail raises ArgumentError
# where `to_i` would silently accept the digit prefix. Used
# when scripts want explicit "this is base-N input" parsing
# with error reporting.

# 1-arg form (regression check — was already supported).
puts Integer("42")               # 42
puts Integer("-7")               # -7

# Explicit radix.
puts Integer("ff", 16)           # 255
puts Integer("FF", 16)           # 255 (uppercase digits accepted)
puts Integer("11111111", 2)      # 255
puts Integer("777", 8)           # 511
puts Integer("z", 36)            # 35

# Sign.
puts Integer("-ff", 16)          # -255
puts Integer("+ff", 16)          # 255

# Prefix consumption — explicit radix that matches the prefix.
puts Integer("0xff", 16)         # 255
puts Integer("0b1010", 2)        # 10
puts Integer("0o17", 8)          # 15

# Radix 0 = auto-detect via prefix.
puts Integer("0xff", 0)          # 255
puts Integer("0b1010", 0)        # 10
puts Integer("0o17", 0)          # 15
puts Integer("0d42", 0)          # 42
puts Integer("42", 0)            # 42 (no prefix → default base 10)

# Underscore digit separator.
puts Integer("1_000", 10)        # 1000
puts Integer("ff_ff", 16)        # 65535

# Whitespace stripping (leading + trailing).
puts Integer("  42  ", 10)       # 42
puts Integer("  ff", 16)         # 255

# Strict parse: garbage tail raises (the key difference from
# `String#to_i`).
[
  ["garbage", 16],
  ["12abc", 10],
  ["ff", 10],
  ["__1", 10],
  ["1__2", 10],
  ["", 16],
].each do |s, r|
  begin
    Integer(s, r)
    puts "expected raise for (#{s.inspect}, #{r})"
  rescue ArgumentError => e
    puts "AE #{s.inspect} radix #{r}: #{e.message}"
  end
end

# Out-of-range radix.
begin
  Integer("5", 1)
rescue ArgumentError => e
  puts "AE radix-bound: #{e.message}"
end

# Non-String value with explicit radix → CRuby raises
# ArgumentError ("base specified for non string value"), NOT
# TypeError. Mirror that.
begin
  Integer(:sym, 16)
rescue ArgumentError => e
  puts "AE non-str-val: #{e.message}"
end
