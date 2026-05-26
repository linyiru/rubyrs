# `String#to_i(radix)` — Tier 1 base-aware parser, inverse of
# `Integer#to_s(radix)`. Radix 2..=36 parses with that base
# explicitly; radix 0 (the special "auto-detect" form) uses
# the `0x`/`0o`/`0b`/`0d` prefix the source carries. CRuby's
# famous lenient parse rules apply otherwise: optional sign,
# digits, stop at the first non-digit, empty / garbage → 0.

# Hex / binary / octal / base 36.
puts "ff".to_i(16)              # 255
puts "FF".to_i(16)              # 255 (uppercase digits accepted)
puts "11111111".to_i(2)         # 255
puts "777".to_i(8)              # 511
puts "z".to_i(36)               # 35
puts "10".to_i(36)              # 36

# Sign.
puts "-ff".to_i(16)             # -255
puts "+ff".to_i(16)             # 255
puts "-0".to_i(16)              # 0

# Explicit-radix prefix MATCH (the radix matches the prefix —
# the prefix is consumed, the rest is parsed normally).
puts "0xff".to_i(16)            # 255
puts "0b1010".to_i(2)           # 10
puts "0o17".to_i(8)             # 15
puts "0d42".to_i(10)            # 42

# `radix = 0` auto-detect via prefix.
puts "0xff".to_i(0)             # 255
puts "0b1010".to_i(0)           # 10
puts "0o17".to_i(0)             # 15
puts "0d42".to_i(0)             # 42
puts "42".to_i(0)               # 42 (no prefix → default base 10)

# Leniency: stop at first non-digit; ignored tail.
puts "abc xyz".to_i(16)         # 2748 (a=10, b=11, c=12; space stops)
puts "1_000".to_i(10)           # 1000 (`_` as digit separator)
puts "ff_ff".to_i(16)           # 65535

# Empty / garbage → 0.
puts "".to_i(16)                # 0
puts "garbage".to_i(10)         # 0
puts "xyz".to_i(8)              # 0 (8 doesn't include any of x/y/z)

# Whitespace skip at the start.
puts "   42".to_i(10)           # 42
puts "   ff".to_i(16)           # 255

# Round trip with `Integer#to_s(radix)` — for the supported
# range, the strings are stable.
ok = (0..255).all? { |n| n.to_s(16).to_i(16) == n }
puts ok                         # true
ok = (0..255).all? { |n| n.to_s(2).to_i(2) == n }
puts ok                         # true

# Out-of-range radix → ArgumentError.
[1, 37, -1].each do |bad|
  begin
    "5".to_i(bad)
    puts "no-raise for #{bad}"
  rescue ArgumentError => e
    puts "AE radix #{bad}: #{e.message}"
  end
end
