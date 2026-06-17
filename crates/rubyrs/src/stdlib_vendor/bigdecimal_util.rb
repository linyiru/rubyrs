# bigdecimal/util — adds `#to_d` (to-BigDecimal) conversions to the
# numeric tower + String/NilClass. rubyrs ships a native BigDecimal;
# this file is the pure-Ruby `util` companion CRuby loads via
# `require "bigdecimal/util"`. money requires it at load (money.rb:2).

class Integer
  def to_d
    BigDecimal(self)
  end
end

class Float
  def to_d(precision = 0)
    BigDecimal(self, precision)
  end
end

class String
  # CRuby uses BigDecimal.interpret_loosely (leading-numeric, lenient).
  # rubyrs has no such entry point, so parse the leading numeric token
  # ourselves and fall back to 0 for a non-numeric string (matching
  # interpret_loosely's "0.4567e2" / "0" leniency).
  def to_d
    m = self.strip[/\A[-+]?\d[\d_]*(?:\.\d+)?(?:[eE][-+]?\d+)?/]
    BigDecimal(m || "0")
  end
end

class BigDecimal
  def to_d
    self
  end

  def to_digits
    if nan? || infinite? || zero?
      to_s
    else
      to_s("F")
    end
  end
end

class Rational
  def to_d(precision = 0)
    BigDecimal(self, precision == 0 ? 16 : precision)
  end
end

class NilClass
  def to_d
    BigDecimal("0")
  end
end
