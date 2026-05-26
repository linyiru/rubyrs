# Rational literals (`0.5r`, `1/3r`, `1000.0r`) — the Prism
# `RationalNode` AST shape that previously raised
# `unsupported node: RationalNode` at parse time, blocking
# any gem helper that uses rational literals as numeric
# constants. Tier 1 maps each rational to its Float
# equivalent (numerator / denominator); exact-fraction
# arithmetic is Tier 2 deferred.
#
# Documented divergence from CRuby (intentional, NOT
# asserted here):
#   - `1000.0r.class` → Float in rubyrs vs Rational in CRuby
#   - `1000.0r.inspect` / `.to_s` → "1000.0" in rubyrs vs
#     "1000/1" in CRuby
# The fixture exercises numeric value (which agrees to Float
# precision) while staying off the display / class paths.
#
# Real-world payoff: msgpack-ruby `lib/msgpack/time.rb` uses
# `nsec / 1000.0r` for Ruby-2.x compatibility; pre-fix
# rubyrs rejected the source entirely at parse, post-fix
# the file parses cleanly (still trips later on `Time.at(...)`
# which is Tier 2 work).

# Numeric value of integer-magnitude rationals.
puts 1000.0r * 1.0          # 1000.0
puts 1.0 / 1000.0r          # 0.001
puts 1.0 / 2.0r             # 0.5

# Negative rationals.
puts -1.5r + 0.0            # -1.5
puts (-1/2r).to_f           # -0.5 (parse `-1/2r` as `-(1/2r)`)

# Arithmetic with at least one Float operand promotes to
# Float in BOTH implementations, so these stay byte-identical.
# Pure Int+Rational or Rational+Rational diverge (rubyrs
# stays Float / CRuby keeps Rational); fixture stays off
# those paths.
puts 5.0 - 0.5r             # 4.5
puts (100 * 0.01r).to_f     # 1.0 (force-Float on the rubyrs-Rational result)
puts (0.25r + 0.25r).to_f   # 0.5

# Range/precision: 1/3r as Float can be compared against
# the same direct Float computation.
r = (1/3r).to_f
f = 1.0 / 3.0
puts r == f                 # true (both Float now)

# Mixed-magnitude exactness inside Float range — force Float
# on the rubyrs side to match CRuby's `.to_f` route.
puts (22.0r / 7.0r).to_f    # 3.142857142857143 — pi approx
