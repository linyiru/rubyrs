# Kernel#Integer(str) auto-detects base from the `0x`/`0b`/`0o`/`0d`
# prefix, treats a bare leading `0` as octal, and allows underscore
# separators between digits — matching CRuby. Previously the 1-arg
# form accepted only plain decimal (`parse::<i64>`).
def i(s, *base)
  base.empty? ? (Integer(s) rescue "ERR") : (Integer(s, base[0]) rescue "ERR")
end

# underscores
p i("1_000")          # 1000
p i("1_2_3")          # 123
p i("1__0")           # ERR (double underscore)
p i("_5")             # ERR (leading underscore)
p i("5_")             # ERR (trailing underscore)

# base prefixes (auto-detect)
p i("0b101")          # 5
p i("0B101")          # 5
p i("0o17")           # 15
p i("0xff")           # 255
p i("0XFF")           # 255
p i("0d99")           # 99
p i("0x_ff")          # ERR (underscore right after prefix)
p i("0xa_b")          # 171 (underscore between hex digits)

# leading-zero octal (auto)
p i("010")            # 8
p i("0777")           # 511
p i("08")             # ERR (8 not an octal digit)
p i("0_10")           # 8
p i("00")             # 0
p i("0")              # 0

# sign + whitespace + prefix combos
p i("-0xff")          # -255
p i("  +0b10  ")      # 2
p i("-7")             # -7

# explicit radix still wins (leading 0 is a plain digit)
p i("010", 10)        # 10
p i("ff", 16)         # 255
p i("zz", 36)         # 1295
p i("howdy")          # ERR (rack Lint's SERVER_PORT check)
