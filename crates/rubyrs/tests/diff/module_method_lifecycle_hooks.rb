# Module#method_added(name) / method_removed(name) /
# method_undefined(name) — fire after `def`, `remove_method`,
# `undef_method` on the receiving class/module. Rails / RSpec /
# many DSLs use method_added to auto-wrap freshly-defined
# methods (validation chains, instrumentation, etc.).

# (1) method_added fires after each `def`.
class A
  def self.method_added(name)
    puts "A.method_added(#{name})"
  end
  def foo; end
  def bar(x); end
end

# (2) method_added fires for alias_method too — CRuby parity.
class B
  def self.method_added(name)
    puts "B.method_added(#{name})"
  end
  def foo; "B.foo"; end
  alias_method :bar, :foo
end

# (3) method_removed fires per-removal in arg order.
class C
  def self.method_removed(name)
    puts "C.method_removed(#{name})"
  end
  def foo; end
  def bar; end
  def baz; end
  remove_method :foo, :baz   # fires twice, in order
end

# (4) method_undefined fires per-arg even though rubyrs's
# undef is a Tier-1 no-op for actual dispatch.
class D
  def self.method_undefined(name)
    puts "D.method_undefined(#{name})"
  end
  def foo; end
  def baz; end
  undef_method :foo, :baz
end

# (5) No hook defined — silent no-op (CRuby doesn't raise).
class E
  def foo; end
  def bar; end
  remove_method :bar
end
puts "E done"

# (6) Hook receiver is the class itself — `self == F` inside.
class F
  def self.method_added(name)
    puts "self == F: #{self == F}"
    puts "name.class: #{name.class}"
  end
  def first_method; end
end

# (7) Subclass def fires method_added on the subclass, NOT the
# parent — even when the parent defined the hook.
class GP
  def self.method_added(name)
    puts "GP.method_added(#{name})"
  end
end
class GCh < GP
  def own; end
end

# (8) String args to remove_method / undef_method are also
# routed (rubyrs accepts both Symbol and String).
class H
  def self.method_removed(name); puts "H.method_removed(#{name})"; end
  def self.method_undefined(name); puts "H.method_undefined(#{name})"; end
  def foo; end
  def bar; end
  remove_method "foo"
  def baz; end
  undef_method "baz"
end

# (9) define_method fires method_added too — CRuby parity. Both
# the block-form and 2-arg form route through different install
# paths in rubyrs but both end up firing the hook.
class I
  def self.method_added(name); puts "I.method_added(#{name})"; end
end
I.define_method(:via_block) { "block-form" }
I.define_method(:via_snapshot, I.instance_method(:via_block))
