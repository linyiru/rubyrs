# Object#define_singleton_method — runtime dispatch path for
# the cases the compiler shortcut at compiler.rs:213 doesn't
# catch (dynamic dispatch via __send__, missing block, wrong
# arity, etc.). The static literal form
#   obj.define_singleton_method(:name) { ... }
# already worked via the compiler shortcut before this PR;
# this fixture mainly exercises the runtime arm.

# (1) Dynamic dispatch via __send__ on Value::Object
o = Object.new
puts o.__send__(:define_singleton_method, :hi) { "hi" }
puts o.hi
puts o.singleton_methods.include?(:hi)

# (2) String name accepted (interner intern path)
o2 = Object.new
puts o2.__send__(:define_singleton_method, "str_name") { "str" }
puts o2.str_name

# (3) Class receiver via __send__ — installs into class's
# singleton_methods (== class-level method).
class C; end
puts C.__send__(:define_singleton_method, :cls_hi) { "C.cls_hi" }
puts C.cls_hi
puts C.singleton_methods.include?(:cls_hi)

# (4) Closure captures outer local
counter = 0
o3 = Object.new
o3.define_singleton_method(:bump) { counter += 1 }
o3.bump; o3.bump; o3.bump
puts counter

# (5) Arity errors — match CRuby's ArgumentError surface
begin; Object.new.define_singleton_method; rescue ArgumentError; puts "argerr-0"; end
begin; Object.new.define_singleton_method(:x); rescue ArgumentError; puts "argerr-noblock"; end
begin; Object.new.define_singleton_method(:x, :y, :z) { }; rescue ArgumentError; puts "argerr-3"; end

# (6) TypeError on non-Symbol/String name
begin; Object.new.define_singleton_method(42) { }; rescue TypeError; puts "typeerr-name"; end
begin; Object.new.define_singleton_method(42); rescue TypeError; puts "typeerr-noblock"; end

# (7) Return value is the method name as a Symbol
sym = Object.new.define_singleton_method(:foo) { 1 }
puts sym
puts sym.class.name

# (8) respond_to? on every supported receiver shape
puts Object.new.respond_to?(:define_singleton_method)
puts C.respond_to?(:define_singleton_method)
