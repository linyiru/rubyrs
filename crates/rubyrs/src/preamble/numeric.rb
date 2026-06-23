# Numeric — the shared real-number protocol CRuby defines on
# `Numeric` (complex.c / numeric.c install most of these on the
# common superclass). Every method here treats the receiver as a
# real number (imaginary part 0), so they hold for Integer / Float
# / Rational alike, and `Complex` reopens the ones that differ in
# complex.rb.
#
# IMPORTANT: a Ruby method defined on `Numeric` shadows the native
# primitive arms of its subclasses (Integer/Float/Rational dispatch
# user methods on ancestors ahead of the native fast path). So only
# names with NO native subclass implementation live here; anything a
# subclass already handles natively — `numerator` / `denominator`
# (Rational), `finite?` / `infinite?` (Float), `fdiv` (Integer) — is
# defined per-class below to avoid shadowing (and, for numerator,
# infinite recursion through `to_r`).
class Numeric
  # Complex decomposition of a real number: imaginary part is 0,
  # the real part is the value itself, and it is its own conjugate.
  def real
    self
  end

  def imaginary
    0
  end
  alias imag imaginary

  def conjugate
    self
  end
  alias conj conjugate

  # `real?` — every plain Numeric is real (Complex overrides to false).
  def real?
    true
  end

  # Rectangular / polar coordinates. The phase angle is 0 for a
  # non-negative value and π for a negative one (CRuby returns the
  # integer 0, not 0.0, in the non-negative case).
  def rectangular
    [self, 0]
  end
  alias rect rectangular

  def arg
    self < 0 ? Math::PI : 0
  end
  alias angle arg
  alias phase arg

  def polar
    [abs, arg]
  end

  # Square of the absolute value — exact for Integer/Rational,
  # Float-valued for Float (`self * self` preserves the type).
  def abs2
    self * self
  end

  # `magnitude` is the documented alias of `abs`.
  def magnitude
    abs
  end

  # `integer?` is false for the general Numeric; Integer overrides.
  def integer?
    false
  end
end

class Integer
  def integer?
    true
  end

  # An Integer is already in lowest terms over a denominator of 1.
  def numerator
    self
  end

  def denominator
    1
  end

  # Integers are always finite.
  def finite?
    true
  end

  def infinite?
    nil
  end
end

class Float
  # numerator / denominator route through the exact IEEE fraction.
  # Defined on Float (not Numeric) so `to_r.numerator` reaches
  # Rational's native arm instead of recursing back here.
  def numerator
    to_r.numerator
  end

  def denominator
    to_r.denominator
  end

  # `fdiv` is plain floating-point division for a Float receiver.
  def fdiv(other)
    self / other.to_f
  end
end

class Rational
  # Rationals are always finite (numerator / denominator are exact
  # integers; the native arms already cover numerator/denominator).
  def finite?
    true
  end

  def infinite?
    nil
  end

  # `fdiv` divides the exact value as a Float.
  def fdiv(other)
    to_f / other.to_f
  end

  # `rationalize` — no argument returns self (a Rational is already
  # exact); with an `eps` it returns the simplest Rational within
  # ±|eps|, via CRuby's continued-fraction search (numeric.c's
  # nurat_rationalize). All arithmetic stays exact.
  def rationalize(eps = nil)
    return self if eps.nil?
    e = eps.abs
    a = self - e
    b = self + e
    return self if a == b
    a, b = b, a if a > b
    if a > 0
      __rationalize_internal(a, b)
    elsif b < 0
      -__rationalize_internal(-b, -a)
    else
      Rational(0, 1)
    end
  end

  private

  # Simplest Rational in [a, b] with 0 < a <= b (continued fractions).
  def __rationalize_internal(a, b)
    p0 = 0
    p1 = 1
    q0 = 1
    q1 = 0
    c = a.ceil
    until c <= b
      k = c - 1
      p2 = k * p1 + p0
      q2 = k * q1 + q0
      t = Rational(1, 1) / (b - k)
      b = Rational(1, 1) / (a - k)
      a = t
      p0 = p1
      q0 = q1
      p1 = p2
      q1 = q2
      c = a.ceil
    end
    Rational(c * p1 + p0, c * q1 + q0)
  end
end
