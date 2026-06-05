# `String#index` / `#rindex` return CHARACTER offsets, not byte
# offsets. rubyrs previously returned the Rust `str::find` BYTE
# offset directly, which diverged from CRuby on any multibyte
# string (and made StringIO#gets / File#gets mis-split lines
# whose prefix contained non-ASCII). `String#length` and
# `String#[]` were already char-based, so `index` was the odd
# one out. This fixture pins the char-offset contract.

# Em-dash is 3 UTF-8 bytes / 1 char; "é" is 2 bytes / 1 char;
# CJK "中" is 3 bytes / 1 char. Each forces byte != char.
s = "a—b\ncd\n"        # chars: a — b \n c d \n  (len 7)
puts "len=#{s.length}"
puts "nl1=#{s.index("\n")}"          # char 3 (not byte 5)
puts "nl2=#{s.index("\n", 4)}"       # char 6 (offset is char-based)
puts "b_at=#{s.index("b")}"          # char 2
puts "slice_to_nl=#{s[0..s.index("\n")].inspect}"  # "a—b\n"

# rindex returns the LAST match as a char offset.
t = "α-β-γ"                # α,-,β,-,γ  (len 5)
puts "rlen=#{t.length}"
puts "rfirst=#{t.index("-")}"        # char 1
puts "rlast=#{t.rindex("-")}"        # char 3

# Mixed needle after a multibyte prefix.
u = "héllo wörld héllo"
puts "u_first=#{u.index("héllo")}"   # 0
puts "u_second=#{u.index("héllo", 1)}"  # 12 (char offset)
puts "u_world=#{u.index("wörld")}"   # 6

# CJK — 3-byte chars throughout.
c = "中文a中文b"            # 中,文,a,中,文,b  (len 6)
puts "c_a=#{c.index("a")}"           # char 2
puts "c_b=#{c.index("b")}"           # char 5
puts "c_second_zhong=#{c.index("中", 1)}"  # char 3

# Empty needle at a multibyte boundary returns the char offset.
puts "empty_mid=#{"a—b".index("", 2)}"   # 2

# Not found → nil, multibyte unaffected.
puts "missing=#{"a—b".index("z").inspect}"

# Slicing chained off index reconstructs correctly (the
# File#gets / StringIO#gets pattern) on a multibyte first line.
buf = "wörd1\nwörd2\n"
pos = 0
nl = buf.index("\n", pos)
line = buf[pos..nl]
puts "line1=#{line.inspect}"         # "wörd1\n"
puts "line1_nl=#{line.end_with?("\n")}"
