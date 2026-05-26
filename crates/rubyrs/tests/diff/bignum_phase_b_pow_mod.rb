# BigInt Phase B.5 — `Integer#pow(exp, mod)` modular exponentiation.
#
# Pre-Phase-B.5 the `pow` method was unsupported — only the `**`
# operator existed, and `5.pow(3, 7)` raised NoMethodError. Now:
# Vm::bigint_primitive's `pow` arm calls Vm::try_bigint_pow_method,
# which delegates the 1-arg form to try_bigint_pow (alias for `**`)
# and routes the 2-arg form through num_bigint::BigInt::modpow.
# No DoS cap is needed for the 2-arg form: modpow never materialises
# the unmodulated intermediate, and the result is bounded by |mod|.

# 1-arg form: alias for **
puts 5.pow(3)
puts 2.pow(10)
puts 2.pow(63)              # i64::MAX + 1 → promotes to BigInt
puts 3.pow(100)             # → BigInt

# 2-arg form: modular exponentiation. Small fast path.
puts 5.pow(3, 7)            # 125 mod 7 = 6
puts 2.pow(10, 1000)        # 1024 mod 1000 = 24
puts 7.pow(8, 5)            # 5764801 mod 5 = 1
puts 5.pow(0, 7)            # 1 mod 7 = 1
puts 0.pow(5, 7)            # 0 mod 7 = 0
puts 0.pow(0, 7)            # 1 mod 7 = 1 (per CRuby's 0**0=1)

# Large exponent: real value of base**exp would be GB-sized, but
# modpow keeps it under |mod|.
puts 2.pow(100, 1_000_000_007)     # standard "prime mod" pattern
puts 2.pow(10_000, 1_000_000_007)
puts 2.pow(1_000_000, 1_000_000_007)

# Negative base. CRuby uses floor-mod (result has same sign as mod).
puts (-5).pow(3, 7)         # -125 floor-mod 7 = 1 (-125 = 7*-18 + 1)
puts (-2).pow(10, 7)        # 1024 mod 7 = 2 (even exponent → positive)
puts (-2).pow(11, 7)        # -2048 mod 7 = 1 (-2048 = 7*-293 + 1)

# Negative modulus: result has same sign as mod (floor-mod).
puts 5.pow(3, -7)           # 125 mod -7 = -1
puts 2.pow(10, -1000)       # 1024 mod -1000 = -976
puts (-5).pow(3, -7)        # -125 mod -7 = -6

# BigInt receiver / exp / mod combinations.
big = 2 ** 100
puts big.pow(2, 1_000_000_007)     # (2^200) mod prime
puts big.pow(big, 1_000_000_007)   # huge^huge mod prime
puts 2.pow(big, 7)                  # 2^(2^100) mod 7 — fits in u32 land via modpow

# Operator method-call shape — `n.send(:pow, ...)`.
puts 5.send(:pow, 3, 7)
puts big.send(:pow, 2, 1000)