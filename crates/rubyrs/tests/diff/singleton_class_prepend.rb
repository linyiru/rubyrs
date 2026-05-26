# `class << self; prepend Mod; end` — singleton-class prepend.
#
# Motivating case: tilt.rb's `finalize!` does
#   class << self
#     prepend(Module.new { def lazy_map(*); raise "..."; end; ... })
#   end
# to install an after-freeze guard layer in front of the class's
# own singleton methods. This fixture covers the named-module
# case; anonymous-module-with-block (`Module.new do ... end`) is
# a separate gap and intentionally not exercised here.

# Basic interception: Wrap#foo wraps C.foo, super defers.
module Wrap
  def foo
    "intercepted-" + super
  end
end

class C
  def self.foo; "C.foo"; end
  def self.bar; "C.bar"; end

  class << self
    prepend Wrap
  end
end

puts C.foo                       # "intercepted-C.foo"
puts C.bar                       # "C.bar" (no prepend match)

# Idempotency — repeated `prepend Wrap` is a no-op (matches CRuby
# and the analogous instance-side `prepend` check).
class D
  def self.x; "D.x"; end
  class << self
    prepend Wrap
    prepend Wrap
  end
end
# Without dedup, super from Wrap#foo would loop (Wrap → Wrap → D).
# `D.x` itself doesn't exercise the prepend, but having two
# `prepend Wrap` statements would corrupt the chain.
# Define `foo` so we can verify the chain still works:
class D
  def self.foo; "D.foo"; end
end
puts D.foo                       # "intercepted-D.foo" (one Wrap, not nested)

# TypeError on non-Module arg matches CRuby phrasing.
class WrongType
  begin
    class << self
      prepend 42
    end
  rescue TypeError => e
    puts "type ok: #{e.message}"
  end
end

# Lookup walks `singleton_prepends` BEFORE `singleton_methods`.
# Define-order: own def AFTER class << self; prepend should still
# put Wrap in front of E.foo.
class E
  class << self
    prepend Wrap
  end
  def self.foo; "E.foo"; end
end
puts E.foo                       # "intercepted-E.foo"

# Inheritance: subclass inherits the prepended chain through
# the superclass walk in `lookup_class_singleton_method`.
class CSub < C
end
puts CSub.foo                    # "intercepted-C.foo" (inherits both Wrap and C.foo)
puts CSub.bar                    # "C.bar"

# Class arg (non-Module) raises TypeError with the
# CRuby-shape "wrong argument type Class (expected Module)"
# message. Without the is_module check, the Class would silently
# get installed into singleton_prepends and corrupt lookup.
class NotAModule; end
class CType
  begin
    class << self
      prepend NotAModule
    end
  rescue TypeError => e
    puts "class-arg ok: #{e.message}"
  end
end

# Transitive idempotency — if a singleton-prepended wrapper
# already includes/prepends Inner, explicitly `prepend Inner`
# should be a no-op (not reorder the chain).
module Inner
  def tag; "inner"; end
end
module Outer
  include Inner                  # Outer transitively reaches Inner
end
class TIdem
  def self.tag; "TIdem"; end
  class << self
    prepend Outer                # singleton_prepends = [Outer], Outer.includes = [Inner]
    prepend Inner                # already reachable via Outer — no-op
  end
end
# Without ancestor-aware dedup, the chain would become
# [Inner, Outer, TIdem] and TIdem.tag → "inner". With dedup it
# stays [Outer, TIdem]; Outer.tag resolves to Inner#tag via
# Outer's includes, so the value is the same — but the order
# matters when both modules define overlapping methods. Easier
# observable: Outer doesn't define `tag` of its own, so
# `TIdem.tag` should still resolve via Outer's include chain
# to Inner#tag.
puts TIdem.tag                   # "inner" (Outer's include of Inner is walked)

# Cross-class idempotency — a subclass explicitly re-prepending
# a wrapper its superclass already installed should be a no-op
# (not reorder or duplicate). Without walking the superclass
# chain in the dedup check, the `prepend Wrap` below would
# silently shadow Super's existing Wrap and corrupt resolution
# (though the visible output is the same here because Wrap is
# the only wrapper — the property locked by this assertion is
# that the dedup walk reaches Super.singleton_prepends).
class IdemSuper
  def self.greet; "super.greet"; end
  class << self
    prepend Wrap
  end
end
class IdemSub < IdemSuper
  class << self
    prepend Wrap                # already in Super's singleton chain — no-op
  end
end
puts IdemSub.greet               # "intercepted-super.greet"
