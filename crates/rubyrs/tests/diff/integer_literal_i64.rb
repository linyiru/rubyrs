# Integer literals are i64-wide. Hex, decimal, and underscore-
# grouped forms all parse to the full i64 range; values beyond
# i64 saturate to i64::MAX / i64::MIN (the subset doesn't
# promote to BigInt — documented in SUBSET.md).

# Hex literals across the 32-bit boundary.
puts 0xff                                    # 255
puts 0xffff                                  # 65535
puts 0xffffffff                              # 4294967295  (was 0)
puts 0xffffffffff                            # 1099511627775
puts 0x1234567890                            # 78187493520
puts 0x0102030405060708                      # 72623859790382856

# Decimal beyond i32.
puts 1234567890                              # 1234567890
puts 72623859790382856                       # 72623859790382856  (was 0)
puts 1_000_000_000_000                       # 1000000000000

# Negative beyond i32.
puts(-1234567890123)                         # -1234567890123
puts(-72623859790382856)                     # -72623859790382856

# i64 bounds — exact MAX / MIN.
puts 9223372036854775807                     # i64::MAX
puts(-9223372036854775808)                   # i64::MIN

# pack/unpack 64-bit Q now round-trips full i64-range values.
big = 72623859790382856
bytes = [big].pack("Q").bytes
puts bytes.inspect                           # [8, 7, 6, 5, 4, 3, 2, 1]
puts bytes.pack("C*").unpack("Q").first      # 72623859790382856
