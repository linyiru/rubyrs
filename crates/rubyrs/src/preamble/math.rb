# Math — the f64 surface over the `__rubyrs_math` host primitive.
# Real singleton-table methods (not dispatch recognisers) on
# purpose: `Math.stub :log10, ...` aliases them like any other
# class method (minitest's module-method stubbing).
#
# Domain contract matches CRuby: sqrt/log-family raise
# Math::DomainError for arguments outside the real domain instead
# of returning NaN. (Math::DomainError itself is pre-installed by
# exceptions.rb.)
module Math
  E  = 2.718281828459045
  PI = 3.141592653589793

  def self.sqrt(x)
    x = x.to_f
    raise Math::DomainError, 'Numerical argument is out of domain - "sqrt"' if x < 0
    __rubyrs_math(:sqrt, x)
  end

  def self.cbrt(x)
    __rubyrs_math(:cbrt, x.to_f)
  end

  def self.exp(x)
    __rubyrs_math(:exp, x.to_f)
  end

  def self.log(x, base = nil)
    x = x.to_f
    raise Math::DomainError, 'Numerical argument is out of domain - "log"' if x < 0
    if base.nil?
      __rubyrs_math(:log, x)
    else
      __rubyrs_math(:log, x, base.to_f)
    end
  end

  def self.log2(x)
    x = x.to_f
    raise Math::DomainError, 'Numerical argument is out of domain - "log2"' if x < 0
    __rubyrs_math(:log2, x)
  end

  def self.log10(x)
    x = x.to_f
    raise Math::DomainError, 'Numerical argument is out of domain - "log10"' if x < 0
    __rubyrs_math(:log10, x)
  end

  def self.sin(x)
    __rubyrs_math(:sin, x.to_f)
  end

  def self.cos(x)
    __rubyrs_math(:cos, x.to_f)
  end

  def self.tan(x)
    __rubyrs_math(:tan, x.to_f)
  end

  def self.asin(x)
    x = x.to_f
    raise Math::DomainError, 'Numerical argument is out of domain - "asin"' if x < -1.0 || x > 1.0
    __rubyrs_math(:asin, x)
  end

  def self.acos(x)
    x = x.to_f
    raise Math::DomainError, 'Numerical argument is out of domain - "acos"' if x < -1.0 || x > 1.0
    __rubyrs_math(:acos, x)
  end

  def self.atan(x)
    __rubyrs_math(:atan, x.to_f)
  end

  def self.atan2(y, x)
    __rubyrs_math(:atan2, y.to_f, x.to_f)
  end

  def self.sinh(x)
    __rubyrs_math(:sinh, x.to_f)
  end

  def self.cosh(x)
    __rubyrs_math(:cosh, x.to_f)
  end

  def self.tanh(x)
    __rubyrs_math(:tanh, x.to_f)
  end

  def self.hypot(x, y)
    __rubyrs_math(:hypot, x.to_f, y.to_f)
  end
end
