# `Module#method_defined?` CRuby-semantics battery (2026-07 fix).
#
# Pre-fix divergences closed here (all probed against ruby 3.4):
#   - PRIVATE records answered true (CRuby: false — public+protected
#     only; `Foo.method_defined?(:initialize)` is false).
#   - The `inherit` second arg was accepted but IGNORED (CRuby:
#     falsy → own method table ONLY — no includes, no PREPENDS, no
#     superclasses; truthiness-evaluated).
#   - The universal public-Object surface (`dup` / `class` / `tap` /
#     `==` / …) answered false on classes (CRuby: true) — and must
#     stay false on bare MODULES (their ancestor chain never reaches
#     Object).
#   - `Integer.method_defined?(:puts)` answered true via the
#     include-private sentinel (CRuby: private Kernel surface is NOT
#     reported).
#   - Non-Sym/Str names fell to NoMethodError (CRuby: TypeError with
#     the INSPECT rendering); wrong arity likewise (ArgumentError
#     "expected 1..2").
# The canonical arm and the walk fast bucket share one helper
# (`class_method_defined` riding the respond_to? memo's new
# RESPOND_PROT_BIT), so this battery pins both paths.

module Mixin
  def from_include; end
end
module Prepended
  def from_prepend; end
end

class Base
  include Mixin
  prepend Prepended
  def pub; end
  def prot; end
  protected :prot
  def priv; end
  private :priv
end
class Child < Base; end

# Visibility: public + protected true, private false.
p Base.method_defined?(:pub)
p Base.method_defined?(:prot)
p Base.method_defined?(:priv)
p Base.method_defined?(:initialize)
p Base.method_defined?(:respond_to_missing?)

# Inherit flag (truthiness-evaluated).
p Child.method_defined?(:pub)
p Child.method_defined?(:pub, true)
p Child.method_defined?(:pub, false)
p Child.method_defined?(:pub, nil)
p Child.method_defined?(:pub, 0)      # 0 is truthy in Ruby
p Base.method_defined?(:pub, false)
p Base.method_defined?(:prot, false)
p Base.method_defined?(:priv, false)

# inherit=false scope: own TABLE only — includes and prepends are
# ancestors, not own methods.
p Base.method_defined?(:from_include)
p Base.method_defined?(:from_include, false)
p Base.method_defined?(:from_prepend)
p Base.method_defined?(:from_prepend, false)

# String names.
p Base.method_defined?("pub")
p Base.method_defined?("priv")
p Base.method_defined?("pub", false)
p Child.method_defined?("pub", false)

# Universal public-Object surface: true on classes, false on modules.
p Base.method_defined?(:dup)
p Base.method_defined?(:class)
p Base.method_defined?(:tap)
p Base.method_defined?(:==)
p Base.method_defined?(:nosuch)
module BareMod
  def own_m; end
end
p BareMod.method_defined?(:own_m)
p BareMod.method_defined?(:own_m, false)
p BareMod.method_defined?(:dup)
p BareMod.method_defined?(:class)

# Primitive-class sentinels: real instance surface true, private
# Kernel surface false.
p Integer.method_defined?(:+)
p Integer.method_defined?(:abs)
p Integer.method_defined?(:puts)
p String.method_defined?(:upcase)
p Kernel.method_defined?(:made_up)

# A user reopen on a primitive class participates (and private
# reopens answer false).
class Integer
  def md_batt_pubm; end
  def md_batt_privm; end
  private :md_batt_privm
end
p Integer.method_defined?(:md_batt_pubm)
p Integer.method_defined?(:md_batt_privm)
class Integer
  remove_method :md_batt_pubm
  remove_method :md_batt_privm
end
p Integer.method_defined?(:md_batt_pubm)

# Errors: TypeError (inspect rendering) and ArgumentError (1..2).
begin
  Base.method_defined?(42)
rescue TypeError => e
  puts "md-typeerr: #{e.message}"
end
begin
  Base.method_defined?(nil, false)
rescue TypeError => e
  puts "md-typeerr-nil: #{e.message}"
end
begin
  Base.method_defined?
rescue ArgumentError => e
  puts "md-arity0: #{e.message}"
end
begin
  Base.method_defined?(:a, true, 1)
rescue ArgumentError => e
  puts "md-arity3: #{e.message}"
end

# The visibility-filtered triplet honours inherit too.
p Base.public_method_defined?(:pub)
p Base.public_method_defined?(:prot)
p Base.protected_method_defined?(:prot)
p Base.private_method_defined?(:priv)
p Base.private_method_defined?(:priv, false)
p Child.private_method_defined?(:priv, false)
p Child.public_method_defined?(:pub, false)
p Child.public_method_defined?(:pub, true)
begin
  Base.public_method_defined?
rescue ArgumentError => e
  puts "pmd-arity0: #{e.message}"
end
begin
  Base.public_method_defined?(nil)
rescue TypeError => e
  puts "pmd-typeerr: #{e.message}"
end

# alias_method'd and attr_accessor methods count as own-table records.
class Base
  attr_accessor :attred
  alias_method :pub2, :pub
end
p Base.method_defined?(:attred)
p Base.method_defined?(:attred=, false)
p Base.method_defined?(:pub2, false)

# undef_method tombstones read as not-defined under both variants.
class Child
  def doomed; end
end
p Child.method_defined?(:doomed)
class Child
  undef_method :doomed
end
p Child.method_defined?(:doomed)
p Child.method_defined?(:doomed, false)
