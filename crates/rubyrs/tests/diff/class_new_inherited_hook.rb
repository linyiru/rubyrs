# `Class.new(Parent)` (the runtime / no-source-form path) must
# fire `Parent.inherited(new_class)` just like the source-form
# `class Sub < Parent`. Pre-fix, the source form fired the hook
# (covered by `class_inherited_hook.rb`) but the runtime form
# silently skipped it — Mustermann's
# `Class.new(const_get(:NodeTranslator))` therefore never
# invoked `NodeTranslator.inherited(subclass)`, which is where
# the parent's `translator` accessor gets stamped onto the
# child. The chain then crashed with
# `NoMethodError: undefined method 'translator' for Class`.
#
# Discovery: P3 Sinatra spike — mustermann/ast/translator.rb's
# `dispatch_table` accessor relies on the inherited hook to
# install per-subclass state.
#
# Implementation: `invoke_inherited_hook` in dispatch.rs is
# called from BOTH `Op::DefClass` (source form) and the
# `Class.new(super_arg)` runtime arm in
# `try_dispatch_class_intrinsics`. The runtime path uses the
# `pre_frames + dispatch_until + pop` pattern (mirroring
# `fire_inclusion_hooks`) so the hook body actually executes
# before the freshly-minted class is returned to the caller.

# Shape 1: bare runtime form — hook fires, callback receives
# the new anonymous class, and `Class.new` returns that same
# class (not nil — the original bug shape).
$log = []
class P1
  def self.inherited(sub)
    $log << "fired"
  end
end
anon1 = Class.new(P1)
puts "log1=#{$log.inspect}"
puts "returned_class1=#{anon1.is_a?(Class)}"
puts "super_chain1=#{anon1.superclass == P1}"

# Shape 2: hook can mutate the subclass before the caller sees
# it (the Mustermann pattern — install per-subclass accessors
# / ivars in `inherited`, then the caller reads them).
class P2
  def self.inherited(sub)
    sub.instance_variable_set(:@stamped, :yes)
  end
end
anon2 = Class.new(P2)
puts "stamped2=#{anon2.instance_variable_get(:@stamped).inspect}"

# Shape 3: source-form parity — both source and runtime forms
# fire the same hook once each.
$count = 0
class P3
  def self.inherited(sub); $count += 1; end
end
class C3 < P3; end          # source form: +1
anon3 = Class.new(P3)        # runtime form: +1
puts "count3=#{$count}"

# Shape 4: no parent — bare `Class.new` (no arg) inherits from
# Object. Object has no user-defined `inherited`, so the
# fast-path skip-on-uninterned-name branch holds and we don't
# crash.
anon4 = Class.new
puts "default_super4=#{anon4.superclass == Object}"

# Shape 5: block form — `Class.new(P) { body }` must still
# fire the hook (the body runs as class_eval after creation).
class P5
  def self.inherited(sub)
    sub.instance_variable_set(:@hook_ran, true)
  end
end
anon5 = Class.new(P5) do
  define_method(:greet) { "hi" }
end
puts "block_hook5=#{anon5.instance_variable_get(:@hook_ran).inspect}"
puts "block_method5=#{anon5.new.greet}"
