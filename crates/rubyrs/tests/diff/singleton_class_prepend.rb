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
    puts "type ok"
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
