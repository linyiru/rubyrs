# A Rational-backed BigDecimal. rubyrs has no native arbitrary-precision
# decimal, but it does have exact `Rational`, so BigDecimal stores its
# value as a Rational (`@r`) and does all arithmetic exactly. The CRuby
# BigDecimal#to_s scientific format ("0.314e1") is NOT reproduced — the
# consumers that drove this in (liquid's numeric filters) always convert
# the result back via #to_f / #to_i, so the decimal text never surfaces;
# a plain #to_s is provided for the rest.
#
# Rational lacks #round/#ceil/#floor in this subset, so those are computed
# here directly from numerator/denominator (the denominator is always
# positive, and Integer#/ floors, so floor division is just `n / d`).
#
# Loaded by `require "bigdecimal"` (always-on extra); also installs the
# Kernel#BigDecimal() conversion function CRuby exposes on require.
class BigDecimal
  def initialize(value)
    @r = BigDecimal.__to_rational(value)
  end

  def self.__to_rational(value)
    case value
    when Rational    then value
    when Integer     then Rational(value, 1)
    when Float       then __parse(value.to_s)
    when BigDecimal  then value.to_r
    when String      then __parse(value)
    else
      if value.respond_to?(:to_r)
        value.to_r
      else
        raise TypeError, "can't convert #{value.class} into BigDecimal"
      end
    end
  end

  # Parse a decimal string ("-3.14159", "100.0", "1.5e3") into a Rational.
  def self.__parse(str)
    s = str.strip
    neg = false
    if s[0] == '-'
      neg = true; s = s[1..]
    elsif s[0] == '+'
      s = s[1..]
    end
    # optional exponent
    ei = s.index('e') || s.index('E')
    exp = 0
    if ei
      exp = s[(ei + 1)..].to_i
      s = s[0...ei]
    end
    # integer / fraction split on the decimal point
    di = s.index('.')
    if di
      int_part = s[0...di]
      frac_part = s[(di + 1)..]
    else
      int_part = s
      frac_part = ''
    end
    int_part = '0' if int_part.empty?
    digits = (int_part + frac_part).to_i
    den = 10 ** frac_part.length
    r = Rational(digits, den)
    r = exp >= 0 ? r * (10 ** exp) : r / (10 ** (0 - exp)) if exp != 0
    neg ? r * -1 : r
  end

  def __coerce_r(o)
    case o
    when BigDecimal then o.to_r
    when Rational   then o
    when Integer    then Rational(o, 1)
    when Float      then BigDecimal.__parse(o.to_s)
    else o.to_r
    end
  end
  private :__coerce_r

  def to_r;  @r; end
  def to_f;  @r.to_f; end
  def to_d;  self; end

  # Truncate toward zero.
  def to_i
    num = @r.numerator
    den = @r.denominator
    q = num.abs / den
    num < 0 ? -q : q
  end
  alias_method :to_int, :to_i
  alias_method :truncate, :to_i

  def +(o); BigDecimal.new(@r + __coerce_r(o)); end
  def -(o); BigDecimal.new(@r - __coerce_r(o)); end
  def *(o); BigDecimal.new(@r * __coerce_r(o)); end
  def /(o); BigDecimal.new(@r / __coerce_r(o)); end
  alias_method :div, :/
  alias_method :quo, :/

  def %(o)
    r = __coerce_r(o)
    q = @r / r
    fl = q.numerator / q.denominator   # floor division (denominator > 0)
    BigDecimal.new(@r - fl * r)
  end
  alias_method :modulo, :%

  def -@; BigDecimal.new(@r * -1); end
  def abs; BigDecimal.new(@r < 0 ? @r * -1 : @r); end

  def <=>(o); @r <=> __coerce_r(o); end
  def ==(o); (@r <=> __coerce_r(o)) == 0; end
  def <(o);  (@r <=> __coerce_r(o)) < 0; end
  def >(o);  (@r <=> __coerce_r(o)) > 0; end
  def <=(o); (@r <=> __coerce_r(o)) <= 0; end
  def >=(o); (@r <=> __coerce_r(o)) >= 0; end

  def zero?; @r == 0; end

  # Round half away from zero to `n` decimal places, returning a
  # BigDecimal (CRuby parity; the caller decides whether to #to_i).
  def round(n = 0)
    ni = n.to_i
    factor = 10 ** ni.abs
    scaled = ni >= 0 ? @r * factor : @r / factor
    num = scaled.numerator
    den = scaled.denominator
    rounded = if num >= 0
      (2 * num + den) / (2 * den)
    else
      -((-2 * num + den) / (2 * den))
    end
    BigDecimal.new(ni >= 0 ? Rational(rounded, factor) : Rational(rounded * factor, 1))
  end

  def floor(n = 0)
    ni = n.to_i
    factor = 10 ** ni.abs
    scaled = ni >= 0 ? @r * factor : @r / factor
    fl = scaled.numerator / scaled.denominator
    BigDecimal.new(ni >= 0 ? Rational(fl, factor) : Rational(fl * factor, 1))
  end

  def ceil(n = 0)
    ni = n.to_i
    factor = 10 ** ni.abs
    scaled = ni >= 0 ? @r * factor : @r / factor
    cl = -((-scaled.numerator) / scaled.denominator)
    BigDecimal.new(ni >= 0 ? Rational(cl, factor) : Rational(cl * factor, 1))
  end

  # Non-scientific decimal text — enough for inspection / interpolation.
  # (CRuby's "0.314e1" form is intentionally not reproduced; see header.)
  def to_s(_fmt = nil)
    @r.to_f.to_s
  end
  def inspect; to_s; end

  def coerce(other)
    [BigDecimal.new(other), self]
  end
end

module Kernel
  # `BigDecimal(value)` — the conversion function CRuby installs when
  # bigdecimal is required. Trailing args (precision / exception:) are
  # accepted and ignored (Rational backing is already exact).
  def BigDecimal(value, *_args, **_opts)
    BigDecimal.new(value)
  end
end
