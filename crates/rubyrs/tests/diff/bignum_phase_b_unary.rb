# BigInt Phase B.2 — unary -@ / +@ / abs.
#
# Pre-Phase-B.2 these methods raised NoMethodError on BigInt
# (lookup.rs didn't whitelist them, dispatch fell through). Int
# receivers used `wrapping_abs` / `wrapping_neg` which silently
# returned `i64::MIN` for `i64::MIN.abs` — CRuby promotes to
# Bignum. Now: numeric_call declines on i64::MIN, dispatch routes
# through Vm::bigint_primitive which calls Vm::try_bigint_unary
# — that materialises the BigInt 2^63 and demotes back to Int on
# fit.

# BigInt receiver — basic cases.
big = 2 ** 100
puts(-big)
puts(+big)
puts big.abs
puts((-big).abs)

# `.send(:-@)` shape — make sure operator method-call dispatch
# matches the unary prefix form.
puts big.send(:-@)
puts((-big).send(:abs))
puts big.send(:+@)

# Int receiver — non-edge cases stay on numeric.rs.
puts((-5).abs)
puts(-(-5))
puts(+5)

# Int i64::MIN edge case — promotes to BigInt 2^63 then stays as
# BigInt since 2^63 > i64::MAX.
min64 = -9_223_372_036_854_775_808
puts min64.abs
puts(-min64)

# Double negation round-trip.
puts(-(-big))
puts(-(-min64))
