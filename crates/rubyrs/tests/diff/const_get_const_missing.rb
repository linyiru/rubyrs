# `const_get(:Absent)` invokes the receiver's `const_missing(sym)` hook
# before raising NameError — the constant-space analogue of
# method_missing. regexp_parser's version_lookup does
# `const_get("V3_4_0")` and relies on const_missing to fall back to the
# nearest defined version constant.
class Foo
  def self.const_missing(name)
    "missing:#{name}"
  end
end
p Foo.const_get(:Bar)        # "missing:Bar"
p Foo.const_get("Baz")       # "missing:Baz"

# A present constant still resolves normally (hook not consulted).
class Foo
  REAL = 42
end
p Foo.const_get(:REAL)       # 42

# Inherited const_missing (defined on a superclass's singleton).
class Base
  def self.const_missing(n); :"fallback_#{n}"; end
end
class Child < Base; end
p Child.const_get(:Whatever) # :fallback_Whatever

# No const_missing defined → NameError as before.
class Plain; end
begin
  Plain.const_get(:Nope)
rescue NameError => e
  puts e.message             # uninitialized constant Plain::Nope
end

# The hook can synthesize and the value flows through.
module Versions
  KNOWN = { "V1" => 1, "V2" => 2 }
  def self.const_missing(name)
    KNOWN[name.to_s] || raise(NameError, "no version #{name}")
  end
end
p Versions.const_get(:V2)    # 2
