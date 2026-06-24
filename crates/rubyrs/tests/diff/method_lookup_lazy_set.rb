# lookup_method_uncached / lookup_class_singleton_method elide their
# per-call visited-HashSet for classes with no includes/prepends (the
# common case) and only allocate it to dedup module diamonds. Exercise
# both the fast (no-module) and slow (diamond/prepend/extend) paths plus
# undef, so the elision can't change resolution order.
class Plain; def hi; :plain; end; end          # no modules -> fast path
p Plain.new.hi
p Object.new.class                              # default new/initialize
class WithInit; def initialize(x); @x = x; end; def x; @x; end; end
p WithInit.new(42).x
module M1; def who; :m1; end; end
module M2; def who; :m2; end; end              # diamond: both define who
class Dia; include M1; include M2; end          # M2 wins (last include)
p Dia.new.who
module Pre; def who; "pre+#{super}"; end; end
class Dia2 < Dia; prepend Pre; end
p Dia2.new.who                                  # prepend over inherited
module Ext; def klass_hi; :ext; end; end
class Host; extend Ext; end                     # singleton extend path
p Host.klass_hi
class Base; def g; :base; end; end
class Sub < Base; def g; :sub; end; undef_method :g; end
begin; Sub.new.g; rescue NoMethodError; p :undefed; end
# deep chain, no modules (fast path repeated)
class A1; def d; :a1; end; end
class B1 < A1; end
class C1 < B1; end
p C1.new.d
