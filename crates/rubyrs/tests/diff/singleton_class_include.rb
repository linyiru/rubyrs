# `singleton_class.include(M)` must ROUTE DISPATCH, not just record
# ancestry. Pre-fix, the explicit-receiver include arm pushed M into
# the eigenclass SHELL's own `includes` chain — which class-method
# lookup (`lookup_class_singleton_method`, walking the REAL class's
# `singleton_includes`/`singleton_prepends`) never consults. So
# `C.singleton_class.ancestors` showed M while `C.m_method`
# NoMethodError'd. The fix redirects the chain write into the real
# class's singleton chains, exactly like `extend` (CRuby-equivalent)
# and like the no-receiver `include` inside a `class << X` body.

module Greeter
  def hello
    "hello from Greeter"
  end
end

# --- class receiver: include on the eigenclass = class methods ------
class Widget; end
Widget.singleton_class.include(Greeter)
p Widget.hello
p Widget.respond_to?(:hello)
p Widget.singleton_class.ancestors.include?(Greeter)
p Widget.singleton_class.include?(Greeter)
p Widget.is_a?(Greeter)
p Widget.methods.include?(:hello)
p Widget.methods(false).include?(:hello) # own-only listing excludes module methods
p Widget.send(:hello)

# instances do NOT get the module's methods (it went to the metaclass)
begin
  Widget.new.hello
rescue NoMethodError
  puts "instance NoMethodError: ok"
end

# --- extend equivalence ---------------------------------------------
class Gadget; end
Gadget.extend(Greeter)
p Gadget.hello
p Gadget.singleton_class.ancestors.include?(Greeter)
p Gadget.methods.include?(:hello)

# idempotency: re-include after extend must not duplicate the entry
Gadget.singleton_class.include(Greeter)
p Gadget.singleton_class.ancestors.count { |a| a == Greeter }

# --- module receiver -------------------------------------------------
module Registry; end
Registry.singleton_class.include(Greeter)
p Registry.hello
p Registry.respond_to?(:hello)

# --- plain-object receiver (heap eigenclass path) --------------------
obj = Object.new
obj.singleton_class.include(Greeter)
p obj.hello
p obj.is_a?(Greeter)
p obj.singleton_class.ancestors.include?(Greeter)

# --- include-after-first-call: method cache must invalidate ----------
class ColdCache; end
begin
  ColdCache.hello
rescue NoMethodError
  puts "before include: NoMethodError"
end
ColdCache.singleton_class.include(Greeter)
p ColdCache.hello

# --- define-after-include: later defs on M must be visible -----------
module LateModule; end
class LateHost; end
LateHost.singleton_class.include(LateModule)
begin
  LateHost.late_method
rescue NoMethodError
  puts "before def: NoMethodError"
end
module LateModule
  def late_method
    "late but present"
  end
end
p LateHost.late_method

# --- prepend on the eigenclass, with super ----------------------------
module Louder
  def shout
    super + "!!"
  end
end

class Speaker
  def self.shout
    "base"
  end
end
Speaker.singleton_class.prepend(Louder)
p Speaker.shout

# --- method_missing via eigenclass include ----------------------------
module Recorder
  def method_missing(name, *args)
    "recorded #{name}"
  end

  def respond_to_missing?(name, include_private = false)
    true
  end
end

class Dsl; end
Dsl.singleton_class.include(Recorder)
p Dsl.anything_goes
p Dsl.respond_to?(:whatever)

# --- included hook fires with the singleton class ---------------------
module Hooked
  def self.included(base)
    puts "included into #{base}"
  end
end

class HookHost; end
HookHost.singleton_class.include(Hooked)
