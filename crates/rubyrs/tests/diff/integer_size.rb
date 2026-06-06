# Integer#size — bytes in the machine representation:
# max(8, ceil(bit_length/8)). Every i64-domain value is one 64-bit
# word (8); Bignums grow by the byte. Discovery: P3 Jekyll spike —
# i18n calls Integer#size.
[0, 1, 255, 256, 2**31, 2**32 - 1, 2**62].each { |n| puts n.size }   # all 8 (fixnum domain)
puts "---"
[2**63, 2**64, 2**65, 2**70, 2**127, 2**128, 2**256].each { |n| puts n.size }  # bignum growth
puts "---"
[-1, -255, -(2**70), -(2**128)].each { |n| puts n.size }            # sign-symmetric
puts 0.size
puts (1 + 1).size
