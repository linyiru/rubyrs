# `const_missing` hook: a missing constant consults
# `Scope.const_missing(:NAME)` (qualified) or `Object.const_missing`
# (bare) before raising NameError.
class WithConst
  def self.const_missing(name); "missing:#{name}"; end
end
p WithConst::FOO
p WithConst::BAR

module Outer
  class Inner
    def self.const_missing(n); "inner:#{n}"; end
  end
end
p Outer::Inner::ZZZ

# no hook anywhere → real NameError
class NoHook; end
begin; NoHook::NOPE; rescue NameError => e; p e.message; end
begin; TOTALLY_MISSING_TOPLEVEL; rescue NameError => e; p e.message; end

# defined constants are unaffected by the hook path
class HasConst
  VALUE = 42
  def self.const_missing(n); "fallback:#{n}"; end
end
p HasConst::VALUE
p HasConst::OTHER

# the hook's return value flows through normally
class Lazy
  def self.const_missing(n); n.to_s.length; end
end
p Lazy::ABCDE
