# Minimal `delegate` stdlib shim — `Delegator`, `SimpleDelegator`,
# and the top-level `DelegateClass(superclass)` factory. Pre-shim
# the kernel stub installs empty Delegator + SimpleDelegator
# shells but `DelegateClass` is a Kernel function not covered
# by the constant-shell mechanism, so Mustermann's
# `mustermann/ast/translator.rb:18`
#   class NodeTranslator < DelegateClass(Node)
# tripped `NoMethodError: undefined method 'DelegateClass'
# for Class` at module-load time.
#
# Strategy: enumerate the public surface implicitly via
# `method_missing` rather than CRuby's eager enumeration of
# `superclass.public_instance_methods`. The Mustermann load
# path only needs the factory to RETURN A SUBCLASSABLE Class;
# the actual delegation semantics fire only inside the gem's
# render-time methods (which Sinatra load doesn't reach).

class Delegator
  def initialize(obj)
    __setobj__(obj)
  end

  def __getobj__
    @delegate_obj
  end

  def __setobj__(obj)
    @delegate_obj = obj
  end

  # `method_missing`-based forwarding — fires for every method
  # not explicitly defined on the Delegator subclass. The
  # captured object handles the call. CRuby's Delegator
  # enumerates `__getobj__.class.public_instance_methods` at
  # define_method time which is sharper for `respond_to?`
  # checks but heavier at class-build time; the load surface
  # we exercise doesn't depend on the distinction.
  def method_missing(name, *args, &blk)
    target = __getobj__
    if target.nil?
      super
    else
      target.__send__(name, *args, &blk)
    end
  end

  def respond_to_missing?(name, include_private = false)
    target = __getobj__
    target.respond_to?(name, include_private) if !target.nil?
  end
end

class SimpleDelegator < Delegator
  # SimpleDelegator is Delegator with the default
  # `__getobj__`/`__setobj__` ivar-backed storage; the
  # inherited methods already do that, so nothing more is
  # needed here for the load surface.
end

# Kernel-level `DelegateClass(superclass)` — returns a new
# Class subclassing Delegator. Mustermann uses
# `class NodeTranslator < DelegateClass(Node)` so the
# returned class must be valid as a `<` superclass. The
# `_superclass_capture` ivar carries the original class for
# any future `super(...).public_instance_methods`-style
# introspection.
def DelegateClass(_superclass)
  # CRuby returns a freshly-built `Class.new(Delegator)` per
  # call, enumerating the superclass's public instance methods
  # into explicit delegates. rubyrs's dynamic-superclass-
  # expression dispatch (`class X < SomeExpr()`) doesn't yet
  # walk a `Class.new(Delegator)`-returned class's
  # method_missing for subclass instances (probe shape:
  # `class C < factory_returning_Class.new(P).new`'s `C.new.zzz`
  # raises NoMethodError instead of routing through P's
  # method_missing). Documented divergence.
  #
  # Return `Delegator` itself instead — the spike's load path
  # (`class NodeTranslator < DelegateClass(Node)`) effectively
  # becomes `class NodeTranslator < Delegator`. NodeTranslator
  # instances inherit Delegator's method_missing-based
  # forwarding directly, which is the only surface the gem-
  # load path exercises. Per-`DelegateClass`-call class
  # identity is lost — multiple `DelegateClass(X)` calls all
  # alias to the same Delegator — acceptable for the load
  # surface; would matter if a gem introspected
  # `Foo.superclass.equal?(DelegateClass(X))` (none in the
  # spike chain do).
  Delegator
end
