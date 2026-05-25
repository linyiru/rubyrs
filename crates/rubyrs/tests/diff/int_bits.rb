# Binary ops
puts 0b1100 & 0b1010      # AND
puts 0b1100 | 0b1010      # OR
puts 0b1100 ^ 0b1010      # XOR

# Shifts — positive count
puts 1 << 0
puts 1 << 4
puts 1 << 8
puts 256 >> 1
puts 256 >> 8

# Shifts — negative count flips direction (CRuby semantics)
puts 1 << -2              # 0 (= 1 >> 2)
puts 8 >> -1              # 16 (= 8 << 1)

# Unary not (~) — i64 truncation matches CRuby for small numbers
puts ~0
puts ~5
puts ~(-1)

# Signed right-shift sign-extends
puts (-1) >> 1
puts (-8) >> 2

# Masks — common idioms
n = 0xABCD
puts(n & 0xFF)            # 205
puts((n >> 8) & 0xFF)     # 171
puts(n | 0xF000)          # 0xFBCD == 64461

# Bit set / clear / toggle
flag = 0b0000
flag = flag | (1 << 3)    # set bit 3
puts flag                  # 8
flag = flag & ~(1 << 3)   # clear bit 3
puts flag                  # 0
flag = flag ^ (1 << 2)    # toggle bit 2
puts flag                  # 4

# Chained — combining
puts((0xF0 | 0x0F) & 0x33)

# Inside a method
def low_byte(n)
  n & 0xFF
end
puts low_byte(0x1234)
puts low_byte(255)

# respond_to?
puts 5.respond_to?(:&)
puts 5.respond_to?(:|)
puts 5.respond_to?(:^)
puts 5.respond_to?(:<<)
puts 5.respond_to?(:>>)
puts 5.respond_to?(:~)
