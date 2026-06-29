# Complex — Ruby's complex-number type (`a + bi`).
#
# Implemented in pure Ruby (Tier 3 shape) rather than as a VM value
# type like Rational: Complex is far less perf-critical, and the
# preamble can reopen the built-in numerics for `to_c` + hook
# `Kernel#Complex` exactly as CRuby's `complex.c` exposes them. The
# built-in numeric LHS reaches `Complex#coerce` through the VM's
# numeric coerce-protocol fallback (`try_numeric_coerce_fallback`),
# so `1 + Complex(0, 1)` works.
#
# Components are kept as whatever Numeric the caller supplied
# (Integer / Float / Rational), matching CRuby — `Complex(1, 2)` has
# Integer parts, `Complex(1.0, 2)` has a Float real part.

class Complex < Numeric
  attr_reader :real, :imaginary
  alias imag imaginary

  # Construct from a real + imaginary part. `Complex.rectangular`
  # / `Complex.rect` are the documented factory names; `new` is
  # private in CRuby but we leave it usable for the preamble.
  def initialize(real, imaginary = 0)
    @real = real
    @imaginary = imaginary
  end

  class << self
    alias rectangular new
    alias rect new
    # `Complex.polar(abs, arg=0)` → abs*(cos(arg) + i*sin(arg)).
    def polar(abs, arg = 0)
      new(abs * Math.cos(arg), abs * Math.sin(arg))
    end
  end

  # The coerce protocol: a built-in numeric on the LHS becomes a
  # Complex so the arithmetic re-dispatches to Complex's operators.
  def coerce(other)
    case other
    when Complex
      [other, self]
    when Numeric
      [Complex.rectangular(other, 0), self]
    else
      raise TypeError, "#{other.class} can't be coerced into Complex"
    end
  end

  def +(other)
    parts = __parts(other)
    return __coerce_apply(:+, other) if parts.nil?
    a, b = parts
    Complex.rectangular(@real + a, @imaginary + b)
  end

  def -(other)
    parts = __parts(other)
    return __coerce_apply(:-, other) if parts.nil?
    a, b = parts
    Complex.rectangular(@real - a, @imaginary - b)
  end

  def *(other)
    parts = __parts(other)
    return __coerce_apply(:*, other) if parts.nil?
    a, b = parts
    Complex.rectangular(@real * a - @imaginary * b,
                        @real * b + @imaginary * a)
  end

  def /(other)
    parts = __parts(other)
    return __coerce_apply(:/, other) if parts.nil?
    a, b = parts
    if b == 0 && a != 0
      # Pure-real divisor — divide each component, preserving the
      # component type (Integer/Integer stays Rational-ish via /).
      Complex.rectangular(@real / a, @imaginary / a)
    else
      denom = a * a + b * b
      Complex.rectangular((@real * a + @imaginary * b).quo(denom),
                          (@imaginary * a - @real * b).quo(denom))
    end
  end
  alias quo /

  def **(other)
    if other.is_a?(Integer)
      if other == 0
        Complex.rectangular(1, 0)
      elsif other > 0
        result = self
        (other - 1).times { result *= self }
        result
      else
        Complex.rectangular(1, 0) / (self ** -other)
      end
    else
      # Non-integer exponent → polar form via Math.
      r = abs ** other
      theta = arg * other
      Complex.rectangular(r * Math.cos(theta), r * Math.sin(theta))
    end
  end

  def -@
    Complex.rectangular(-@real, -@imaginary)
  end

  def +@
    self
  end

  def ==(other)
    case other
    when Complex
      @real == other.real && @imaginary == other.imaginary
    when Numeric
      @imaginary == 0 && @real == other
    else
      # Defer to the other object (e.g. `other == self`) only when
      # it isn't a basic mismatch; CRuby returns false otherwise.
      false
    end
  end

  def abs
    Math.hypot(@real, @imaginary)
  end
  alias magnitude abs

  def abs2
    @real * @real + @imaginary * @imaginary
  end

  # Argument (phase angle) in radians.
  def arg
    Math.atan2(@imaginary, @real)
  end
  alias angle arg
  alias phase arg

  def conjugate
    Complex.rectangular(@real, -@imaginary)
  end
  alias conj conjugate

  def polar
    [abs, arg]
  end

  def rectangular
    [@real, @imaginary]
  end
  alias rect rectangular

  # A Complex with a zero imaginary part converts back to a real.
  def real?
    false
  end

  def to_c
    self
  end

  def to_i
    raise RangeError, "can't convert #{self} into Integer" unless @imaginary == 0
    @real.to_i
  end

  def to_f
    raise RangeError, "can't convert #{self} into Float" unless @imaginary == 0
    @real.to_f
  end

  def to_r
    raise RangeError, "can't convert #{self} into Rational" unless @imaginary == 0
    @real.to_r
  end

  def to_s
    "#{@real}#{__imag_str(false)}"
  end

  def inspect
    "(#{@real.inspect}#{__imag_str(true)})"
  end

  def hash
    @real.hash ^ @imaginary.hash
  end
  alias eql? ==

  def fdiv(other)
    Complex.rectangular(@real.fdiv(other), @imaginary.fdiv(other))
  end

  # numerator / denominator clear the fractional parts of BOTH
  # components over their common denominator, matching CRuby's
  # complex.c: `denominator` is the lcm of the two part
  # denominators, `numerator` scales each part up to it.
  def denominator
    @real.denominator.lcm(@imaginary.denominator)
  end

  def numerator
    cd = denominator
    Complex.rectangular(
      @real.numerator * (cd / @real.denominator),
      @imaginary.numerator * (cd / @imaginary.denominator),
    )
  end

  # finite? / infinite? fold over both components: a Complex is
  # finite only when both parts are; infinite? yields 1 if either
  # part is infinite, else nil.
  def finite?
    @real.finite? && @imaginary.finite?
  end

  def infinite?
    (@real.infinite? || @imaginary.infinite?) ? 1 : nil
  end

  def zero?
    @real.zero? && @imaginary.zero?
  end

  def nonzero?
    zero? ? nil : self
  end

  # rationalize only succeeds for a real-valued Complex.
  def rationalize(*args)
    unless @imaginary == 0
      raise RangeError, "can't convert #{self} into Rational"
    end
    @real.rationalize(*args)
  end

  private

  # Decompose `other` into [real, imag] parts when it is a number,
  # else nil so the caller runs the coerce protocol.
  def __parts(other)
    case other
    when Complex then [other.real, other.imaginary]
    when Numeric then [other, 0]
    else nil
    end
  end

  # Run the coerce protocol against a non-numeric operand.
  def __coerce_apply(op, other)
    if other.respond_to?(:coerce)
      a, b = other.coerce(self)
      a.send(op, b)
    else
      raise TypeError, "#{other.class} can't be coerced into Complex"
    end
  end

  # Imaginary-part rendering: sign + magnitude + "i". Integer/Float
  # magnitudes render bare (`+4i`, `+2.5i`); other Numerics (Rational)
  # render with a `*` separator and — under inspect — their own
  # parenthesised form (`+(2/25)*i`), matching CRuby's complex.c.
  def __imag_str(use_inspect)
    im = @imaginary
    negative = im < 0
    sign = negative ? "-" : "+"
    # `* -1` rather than `.abs`/`-@` so this works for any Numeric
    # component (rubyrs Rational lacks both).
    mag = negative ? im * -1 : im
    body = use_inspect ? mag.inspect : mag.to_s
    star = (im.is_a?(Integer) || im.is_a?(Float)) ? "" : "*"
    "#{sign}#{body}#{star}i"
  end
end

module Kernel
  # `Complex(real, imag = 0)` — the conversion function. A single
  # Complex arg passes through; a String parses (minimal support).
  def Complex(real, imag = 0, exception: true)
    if real.is_a?(Complex) && imag == 0
      real
    elsif real.is_a?(String)
      __parse_complex(real)
    else
      Complex.rectangular(real, imag)
    end
  end
  module_function :Complex

  private

  def __parse_complex(str)
    s = str.strip
    # Forms: "a+bi", "a-bi", "bi", "a". Minimal but covers the
    # common cases; falls back to a real-only Complex.
    # Built via `Regexp.new` (not a `/…/` literal) so this preamble still
    # PARSES in a `--no-default-features` (regex-off) build — a `/…/` literal
    # is a load-time SyntaxError there (ADR 0017 Rule 3). String parsing then
    # degrades to a runtime error if regex is absent, instead of crashing the
    # whole runtime at preamble load (issue: Windows/wasm no-regex smoke).
    if (m = s.match(Regexp.new("\\A([+-]?\\d+(?:\\.\\d+)?)?([+-]\\d+(?:\\.\\d+)?)?i\\z")))
      real = m[1] ? __num(m[1]) : 0
      imag = m[2] ? __num(m[2]) : (m[1] ? 1 : __num(m[1] || "1"))
      Complex.rectangular(real, imag)
    elsif s.match?(Regexp.new("\\A[+-]?\\d+(?:\\.\\d+)?\\z"))
      Complex.rectangular(__num(s), 0)
    else
      raise ArgumentError, "invalid value for convert(): #{str.inspect}"
    end
  end

  def __num(s)
    s.include?(".") ? s.to_f : s.to_i
  end
  module_function :__parse_complex, :__num
end

class Integer
  def to_c
    Complex.rectangular(self, 0)
  end
  def i
    Complex.rectangular(0, self)
  end
  # `Integer#quo` — exact division: Integer/Integer → Rational,
  # Integer/Float → Float, Integer/Rational → Rational. Built on
  # Rational arithmetic so the type-promotion matches CRuby.
  def quo(other)
    Rational(self, 1) / other
  end
end

class Float
  def to_c
    Complex.rectangular(self, 0)
  end
  def i
    Complex.rectangular(0, self)
  end
  # `Float#quo` — always a Float (float contaminates the result).
  def quo(other)
    self / other
  end
end

class Rational
  def to_c
    Complex.rectangular(self, 0)
  end
  # `Rational#quo` is the same as `/` (already exact / float-aware).
  def quo(other)
    self / other
  end
end

class NilClass
  def to_c
    Complex.rectangular(0, 0)
  end
end
