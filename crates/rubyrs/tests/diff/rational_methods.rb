# Rational instance methods beyond the arithmetic/comparison core:
# unary (abs / magnitude / -@ / +@ / abs2), sign predicates
# (zero? / nonzero? / positive? / negative?), coerce, and the
# rounding family (floor / ceil / round / truncate) with optional
# decimal precision and round's `half:` tie-break keyword.
#
# Every line below is byte-stable across CRuby and rubyrs because
# the values are exact (no Float display) — Rational results
# `inspect` as `(n/d)` identically on both. This fixture is the
# regression guard for the native Rational dispatch arms in
# vm/dispatch.rs.

# --- unary ---
p Rational(-3, 4).abs
p Rational(3, 4).abs
p Rational(-5, 3).magnitude
p(-Rational(3, 4))
p(-Rational(-3, 4))
p(+Rational(3, 4))
p Rational(3, 4).abs2
p Rational(-2, 7).abs2

# --- sign predicates ---
p Rational(0, 1).zero?
p Rational(1, 2).zero?
p Rational(0, 1).nonzero?
p Rational(4, 2).nonzero?
p Rational(1, 2).positive?
p Rational(-1, 2).positive?
p Rational(-1, 2).negative?
p Rational(1, 2).negative?

# --- coerce ---
p Rational(3, 4).coerce(2)
p Rational(3, 4).coerce(Rational(1, 2))

# --- floor / ceil / truncate (no arg → Integer) ---
p Rational(7, 2).floor
p Rational(-7, 2).floor
p Rational(7, 2).ceil
p Rational(-7, 2).ceil
p Rational(7, 2).truncate
p Rational(-7, 2).truncate

# --- round (no arg → Integer, half-away-from-zero default) ---
p Rational(7, 2).round
p Rational(5, 2).round
p Rational(-5, 2).round
p Rational(1, 3).round

# --- precision: ndigits > 0 → Rational ---
p Rational(1, 3).floor(2)
p Rational(1, 3).ceil(2)
p Rational(-1, 3).truncate(2)
p Rational(1, 3).round(2)
p Rational(10, 3).round(5)

# --- precision: ndigits < 0 → Integer ---
p Rational(1234, 1).floor(-2)
p Rational(1234, 1).ceil(-2)
p Rational(1234, 1).round(-2)
p Rational(99, 1).round(-2)
p Rational(150, 1).round(-2)

# --- round half: keyword ---
p Rational(1, 2).round(half: :up)
p Rational(1, 2).round(half: :down)
p Rational(1, 2).round(half: :even)
p Rational(3, 2).round(half: :even)
p Rational(5, 2).round(half: :even)
