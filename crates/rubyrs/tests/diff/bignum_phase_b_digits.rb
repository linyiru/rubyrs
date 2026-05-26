# BigInt Phase B.5 — `Integer#digits([base])` + `Integer#bit_length`
# on BigInt.
#
# `digits` builds an Array of digits (least-significant first) in
# the given base (default 10). `bit_length` returns the bit position
# of the most-significant 1 bit for non-negatives, or the position
# of the most-significant 0 bit for negatives (two's-complement
# semantics — equivalent to `bit_length(~n) = bit_length(-n - 1)`).
#
# Negative-receiver `.digits` raises `Math::DomainError` in CRuby;
# rubyrs substitutes ArgumentError per the documented subset
# pattern (dispatch.rs:2402-2403). That case lives in
# `tests/embed.rs` not here, since the error-class names diverge.

# `digits` — small cases stay as Int, no BigInt involvement.
p 0.digits
p 1.digits
p 12345.digits
p 12345.digits(16)
p 255.digits(16)
p 12345.digits(2)

# `digits` — BigInt receiver. Pre-Phase-B.5 these would
# NoMethodError. Verify a few entries + length (full array would
# be many lines and arr[i, n] isn't supported here).
big = 2 ** 100
ds = big.digits
puts "base10[0]=#{ds[0]} base10[1]=#{ds[1]} len=#{ds.length}"
ds16 = big.digits(16)
puts "hex[0]=#{ds16[0]} hex[len-1]=#{ds16[ds16.length - 1]} len=#{ds16.length}"
ds2 = big.digits(2)
puts "bin[0]=#{ds2[0]} bin[len-1]=#{ds2[ds2.length - 1]} len=#{ds2.length}"

# `bit_length` — Int side (already shipped, here for parity).
p 0.bit_length
p 1.bit_length
p 255.bit_length
p (-1).bit_length
p (-256).bit_length

# `bit_length` — BigInt receiver. New in Phase B.5.
p big.bit_length          # 2**100 has 101 bits
p (-big).bit_length       # bit_length(-2^100) = bit_length(2^100 - 1) = 100
p (big + 1).bit_length    # 2**100 + 1 still 101 bits
p (big * 256).bit_length  # 2**108 → 109 bits

# Validation errors that DO match CRuby exactly (no error-class
# divergence).
begin; 5.digits(1); rescue ArgumentError => e; puts "r1: #{e.message}"; end
begin; 5.digits(0); rescue ArgumentError => e; puts "r0: #{e.message}"; end
begin; 5.digits(-2); rescue ArgumentError => e; puts "rneg: #{e.message}"; end
begin; 5.digits("x"); rescue TypeError => e; puts "str: #{e.message}"; end
