# `String#to_i` — BigInt promotion + the full lenient rule set,
# probed against CRuby 3.4 (the shared `str2int` scanner). The
# historical bug: digits folded in i64 with wrapping arithmetic, so
# `"18446744073709551616".to_i` was 0 and a 30-digit string came
# back as a wrapped negative — silent data corruption.

# --- exact BigInt promotion (the core fix) ---
puts "18446744073709551616".to_i               # 2**64 exactly
puts "123456789012345678901234567890".to_i
puts "-123456789012345678901234567890".to_i
puts "1_000_000_000_000_000_000_000".to_i      # underscores + big
puts(("9" * 100).to_i)
puts "18446744073709551616".to_i.class         # Integer (unified)

# --- i64 boundary discipline: MIN/MAX stay Small, ±1 promotes ---
puts "9223372036854775806".to_i
puts "9223372036854775807".to_i                # i64::MAX
puts "9223372036854775808".to_i                # MAX+1 → promote
puts "-9223372036854775807".to_i
puts "-9223372036854775808".to_i               # exactly i64::MIN
puts "-9223372036854775809".to_i               # MIN-1 → promote
puts "8000000000000000".to_i(16)               # 2**63 via hex
puts "-8000000000000000".to_i(16)              # i64::MIN via hex
puts "-8000000000000001".to_i(16)
puts "ffffffffffffffffff".to_i(16)             # 72 bits
puts(("1" * 67).to_i(2))

# --- BigInt results behave like Integers ---
big = "18446744073709551616".to_i
puts big + 1
puts big - 1
puts big == 2 ** 64
puts big <=> 2 ** 63
puts big / 2
puts big.to_s
puts big.to_s(16)
puts((2 ** 100).to_s(16).to_i(16) == 2 ** 100) # round trip
puts((2 ** 100).to_s(36).to_i(36) == 2 ** 100)

# --- underscores (probed: `_` only BETWEEN digits) ---
puts "1_0".to_i
puts "1__0".to_i        # stops at the ill-placed 2nd underscore → 1
puts "1_".to_i          # trailing _ → 1
puts "_1".to_i          # leading _ → 0
puts "0_1_0".to_i       # base 10: 10
puts "f_f".to_i(16)
puts "0x_10".to_i(16)   # _ right after prefix → 0
puts "0_1_0".to_i(0)    # auto: octal → 8
puts "0__10".to_i(0)    # double _ after octal-0 → 0
puts "1__0".to_i(10)

# --- whitespace: ASCII-only skip (NOT unicode) ---
puts "  42".to_i
puts "\t\n\v\f\r 42".to_i
puts " 42".to_i          # NBSP (U+00A0) is NOT whitespace → 0
puts "42 ".to_i          # trailing garbage ignored anyway
puts "  -42abc".to_i

# --- signs ---
puts "+42".to_i
puts "-42".to_i
puts "- 42".to_i         # space after sign → 0
puts "--42".to_i
puts "+-42".to_i

# --- prefixes: explicit base consumes ONLY a matching prefix ---
puts "0x10".to_i         # no-arg = base 10: stops at 'x' → 0
puts "0d19".to_i         # but 0d IS the base-10 prefix → 19
puts "0D19".to_i
puts "019".to_i          # leading 0 is NOT octal for explicit 10
puts "0x10".to_i(16)
puts "0X10".to_i(16)
puts "0x10".to_i(10)     # mismatched → 0
puts "0b10".to_i(16)     # 'b' is a hex DIGIT → 0xb10
puts "0b10".to_i(36)     # 'b' is a base-36 digit
puts "0x10".to_i(2)      # 'x' invalid in binary → 0
puts "0b101".to_i(2)
puts "0o17".to_i(8)
puts "017".to_i(8)
puts "0x0x10".to_i(16)   # prefix consumed once → 0 then stop

# --- base 0 auto-detect ---
puts "0x10".to_i(0)
puts "0b10".to_i(0)
puts "0o17".to_i(0)
puts "0d42".to_i(0)
puts "010".to_i(0)       # bare leading 0 → octal
puts "08".to_i(0)        # 8 is no octal digit → 0
puts "-0x10".to_i(0)
puts "0b".to_i(0)        # prefix but no digits → 0
puts "0".to_i(0)
puts "00".to_i(0)

# --- no digits / garbage → 0 ---
puts "".to_i
puts "abc".to_i
puts "0x".to_i(16)
puts "0xg".to_i(16)
puts "４２".to_i          # fullwidth digits are not digits
puts "１".to_i(36)

# --- invalid radix raises ---
[1, 37, -1, -16].each do |bad|
  begin
    "5".to_i(bad)
    puts "no-raise #{bad}"
  rescue ArgumentError => e
    puts "AE: #{e.message}"
  end
end

# --- block-form call (block ignored, big still exact) ---
puts("18446744073709551616".to_i { :ignored })

# --- respond_to? surface ---
puts "x".respond_to?(:to_i)
