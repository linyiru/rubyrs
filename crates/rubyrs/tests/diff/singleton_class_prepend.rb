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
#
# Observability: Outer wraps super so the chain order is visible
# in the output. With dedup, chain at TIdem = [Outer, TIdem, ...
# Inner via Outer's includes], so Outer#tag's super resolves to
# Inner#tag → "outer-inner". WITHOUT dedup, `prepend Inner` would
# put Inner ahead of Outer, breaking Outer's super-chain and
# producing just "outer-<NoMethodError>" or "inner" depending on
# where super lands. The wrapped output catches the regression
# the previous "inner" alone couldn't.
module Inner
  def tag; "inner"; end
end
module Outer
  include Inner                  # Outer transitively reaches Inner
  def tag; "outer-" + super; end # wraps Inner#tag via the include chain
end
class TIdem
  def self.tag; "TIdem"; end
  class << self
    prepend Outer                # singleton_prepends = [Outer], Outer.includes = [Inner]
    prepend Inner                # already reachable via Outer — no-op
  end
end
puts TIdem.tag                   # "outer-inner" (Outer wraps, super hits Inner via includes)

# Note: cross-class singleton-prepend (the same Wrap module
# prepended via `class << self` on BOTH a class and its
# subclass) is intentionally NOT exercised here. In CRuby each
# eigenclass holds an independent IClass per prepend, so the
# chain contains Wrap twice and `Sub.foo` double-wraps. rubyrs's
# chain representation dedupes by Module identity (no IClass
# layer), so the second `prepend Wrap` wouldn't be observable
# in either direction without a much deeper refactor — out of
# scope for this PR. The local-class dedup above already locks
# the dogfood case (tilt's `class << self; prepend(Module.new)`).
