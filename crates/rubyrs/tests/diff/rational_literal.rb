# Rational literals (`0.5r`, `1/3r`, `1000.0r`) — Phase C.4.4
# wires `RationalNode` to a real `Value::Rational` (replacing
# the pre-C.4.4 lowering-to-Float hack). Both `class` and
# arithmetic now match CRuby exactly. This fixture stays in
# place as a regression guard against the old Float-lowering
# behavior creeping back: each line is byte-stable across
# CRuby and rubyrs because either (a) an explicit `.to_f`
# coerces the Rational to Float for display, or (b) a Float
# operand on the other side of the expression promotes the
# result to Float (e.g. `1000.0r * 1.0`). Pure Rational ×
# Rational paths now stay Rational on both sides — the fixture
# avoids displaying those directly to keep the output stable.
#
# See `spec/ruby/rational_literal_spec.rb` for the class /
# numerator / denominator assertions and `tests/embed/numeric.rs`
# `rational_phase_c4_4_literal_and_pow` for the embed surface.

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
