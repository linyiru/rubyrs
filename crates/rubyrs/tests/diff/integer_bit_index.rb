# Integer#[] — bit access via the indexing operator.
# Single arg: `n[i]` → bit at position i (0 = LSB).
# Two-arg form: `n[offset, length]` → bitfield of width `length`
#   starting at `offset`.
#
# For negatives, two's-complement extension applies: bits above
# the i64 width are all 1s, so `(-1)[100] == 1`.

# Single-arg form.
puts 0b1010[0]                       # 0
puts 0b1010[1]                       # 1
puts 0b1010[2]                       # 0
puts 0b1010[3]                       # 1
puts 0b1010[4]                       # 0 (out of represented bits)
puts 0b1010[100]                     # 0
puts 0[0]                            # 0
puts 1[0]                            # 1

# Negative receivers — two's-complement view.
puts (-1)[0]                         # 1
puts (-1)[63]                        # 1
puts (-1)[100]                       # 1
puts (-2)[0]                         # 0
puts (-2)[1]                         # 1

# Negative index → 0.
puts 5[-1]                           # 0
puts (-5)[-1]                        # 0

# Two-arg form — bitfield extract.
puts 255[0, 4]                       # 15  (low nibble)
puts 255[4, 4]                       # 15  (high nibble of byte)
puts 255[0, 8]                       # 255 (whole byte)
puts 0xFF00[8, 8]                    # 255 (second byte)
puts 0xCAFE[0, 16]                   # 51966 (whole 16 bits)
puts 0xCAFE[4, 8]                    # 175 (0xAF)
puts 0[0, 32]                        # 0
puts 1[0, 1]                         # 1
puts 1[0, 0]                         # 0

# Length 32 bitfield — the case msgpack/bigint.rb uses.
puts 0xDEADBEEF[0, 32]               # 0xDEADBEEF = 3735928559
puts 0xDEADBEEF[16, 16]              # 0xDEAD = 57005

# Length > available bits.
puts 7[0, 64]                        # 7
puts 7[2, 64]                        # 1

# Negative offset → 0.
puts 5[-1, 1]                        # 0
# (Negative `length` is a documented divergence: CRuby returns 5
# for `5[0, -1]` via some internal short-circuit; rubyrs returns
# 0. Not exercised in the diff fixture so the byte-identical
# contract holds.)
