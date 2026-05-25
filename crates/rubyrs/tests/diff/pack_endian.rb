# Pack endian modifiers — `>` / `<` suffixes on S/L/Q (and the
# bare `S` / `L` / `Q` mapping to native = LE on our targets).
# Pre-A6a we only had `n`/`N` (BE 16/32) and `v`/`V` (LE 16/32)
# as discrete directives. msgpack's `lib/msgpack/bigint.rb`
# uses `"CL>*"`, which couldn't parse before this commit.

# 32-bit forms.
puts [0x12345678].pack("L>").bytes.inspect   # [18, 52, 86, 120]
puts [0x12345678].pack("L<").bytes.inspect   # [120, 86, 52, 18]
puts [0x12345678].pack("L").bytes.inspect    # native = LE
puts [0x12345678].pack("N").bytes.inspect    # alias for L>
puts [0x12345678].pack("V").bytes.inspect    # alias for L<

# 16-bit forms.
puts [0x1234].pack("S>").bytes.inspect       # [18, 52]
puts [0x1234].pack("S<").bytes.inspect       # [52, 18]
puts [0x1234].pack("n").bytes.inspect
puts [0x1234].pack("v").bytes.inspect

# 64-bit forms — `Q>` / `Q<` / `Q` (native) / `q>` (signed BE).
puts [1].pack("Q>").bytes.inspect            # [0,0,0,0,0,0,0,1]
puts [1].pack("Q<").bytes.inspect            # [1,0,0,0,0,0,0,0]
puts [1].pack("Q").bytes.inspect             # native = LE
puts [-1].pack("q>").bytes.inspect           # all-ones (8 × 255)

# Counts + repeat.
puts [1, 2, 3].pack("L>*").bytes.inspect     # 3 × 4-byte BE
puts [0xAA, 0x1234].pack("CL>").bytes.inspect # mix: C then L>
puts [0xAA, 0x1234].pack("CS>").bytes.inspect

# msgpack-bigint-shaped: byte tag + many u32 BE limbs.
puts [0, 0xDEADBEEF, 0xCAFEBABE].pack("CL>*").bytes.inspect
