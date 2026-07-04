# `String#hex` / `String#oct` — probed against CRuby 3.4.
# `hex` ≡ the lenient scan with base 16 (optional `0x`/`0X` prefix,
# sign, `_` between digits, garbage → 0). `oct` is the lenient scan
# with CRuby's NEGATIVE base -8: default octal, but any
# `0b`/`0x`/`0o`/`0d` prefix OVERRIDES the base. Both promote to
# exact BigInt past i64 range.

# --- hex ---
puts "0x10".hex
puts "0X10".hex
puts "10".hex
puts "ff".hex
puts "FF".hex
puts "-ff".hex
puts "+ff".hex
puts "f_f".hex
puts "_ff".hex          # leading underscore → 0
puts "0xgg".hex         # prefix consumed, no digits → 0
puts "gg".hex
puts "".hex
puts "x10".hex
puts " 10".hex          # ASCII whitespace skipped
puts "\t-f".hex
puts "0x_10".hex        # underscore right after prefix → 0

# hex → BigInt promotion
puts "ffffffffffffffffff".hex
puts "8000000000000000".hex            # 2**63 → still exact
puts "-8000000000000000".hex           # i64::MIN → Small
puts "-8000000000000001".hex           # → BigInt
puts(("f" * 40).hex)
puts "ffffffffffffffffff".hex.class

# --- oct ---
puts "10".oct
puts "010".oct
puts "-777".oct
puts "777".oct
puts "8".oct            # 8 is no octal digit → 0
puts "08".oct
puts "".oct
puts "0".oct
puts " 10".oct
puts "0_10".oct
puts "0_x10".oct        # `_x` stops the scan → 0

# oct prefix override (the negative-base behavior)
puts "0b10".oct
puts "0B10".oct
puts "0x10".oct
puts "0X10".oct
puts "0o17".oct
puts "0O17".oct
puts "0d19".oct
puts "0D19".oct
puts "-0b10".oct

# oct → BigInt promotion
puts "777777777777777777777777777777".oct
puts "1000000000000000000000".oct       # 2**63 octal
puts(("7" * 50).oct)
puts "0x10000000000000000".oct          # override to hex, then big

# --- respond_to? surface ---
puts "x".respond_to?(:hex)
puts "x".respond_to?(:oct)

# --- block form (block ignored) ---
puts("ffffffffffffffffff".hex { :ignored })
