# BigInt Phase B.4 — `Integer#to_s(radix)` + sprintf `%x %X %o %b %B`
# for BigInt receivers / args.
#
# Pre-Phase-B.4, `big.to_s(16)` raised NoMethodError (Phase A only
# handled the 0-arg form), and `"%x" % big` raised TypeError
# (coerce_int didn't accept BigInt). Now both shapes work via
# num_bigint's `to_str_radix` on the magnitude, with sign + alt
# prefix applied uniformly with the Int side.
#
# `Integer#to_s(radix)` already returns `-<magnitude>` for
# negative inputs in CRuby (no divergence), so the negative-to_s
# cases below match byte-for-byte and the embed test
# `bigint_to_s_radix_negative_uses_minus_magnitude_form` pins
# that exact behaviour. The `..f` two's-complement divergence
# only applies to `sprintf '%x' % neg` — that case is exercised
# in `tests/embed.rs::sprintf_bigint_radix_negative_uses_minus_magnitude_divergence`
# and excluded from this fixture (byte output differs from CRuby).

# `to_s(radix)` — basic positive cases (byte-identical to CRuby).
big = 2 ** 100
puts big.to_s              # default base 10
puts big.to_s(10)
puts big.to_s(16)
puts big.to_s(2)[0, 40]    # truncate the 101-bit form
puts big.to_s(36)
puts big.to_s(8)
puts (2 ** 63).to_s(16)    # i64::MAX + 1, sanity boundary
puts (2 ** 256 - 1).to_s(16)  # all-1s hex

# `to_s(radix)` via `.send`.
puts big.send(:to_s, 16)

# sprintf `%x %X %o %b %B` on positive BigInt — byte-identical.
puts '%x' % big
puts '%X' % big
puts '%o' % big
puts '%b' % (2 ** 50)
puts '%B' % (2 ** 50)
puts '%#x' % big
puts '%#X' % big
puts '%#o' % big

# Width / precision on hex BigInt.
puts '%020x' % (2 ** 60)
puts '%-30x|' % (2 ** 60)
puts '%.10x' % (2 ** 60)

# Validation errors that DO match CRuby exactly.
begin; big.to_s(1); rescue ArgumentError => e; puts "r1: #{e.message}"; end
begin; big.to_s(37); rescue ArgumentError => e; puts "r37: #{e.message}"; end
begin; big.to_s(-2); rescue ArgumentError => e; puts "rneg: #{e.message}"; end
begin; big.to_s("x"); rescue TypeError => e; puts "str: #{e.message}"; end
