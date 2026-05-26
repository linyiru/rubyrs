# `class Bar` at top level and `module Foo; class Bar; end; end`
# are TWO DISTINCT classes in CRuby — they have independent
# method tables, ivars, and superclass slots. Prior to this
# commit rubyrs used a class table keyed by bare SymId, so the
# two `Bar` defines collapsed into a single shared Class object;
# methods added in one definition would leak into the other and
# `Class#name` would report whichever was assigned first.
#
# Key-by-qualified-name refactor: `Op::DefClass` now keys the
# class table by `qual_id` when one is supplied (i.e. when the
# class is being defined inside a module/class body) and by the
# bare `name_id` only for top-level definitions. Two `class Bar`
# in different scopes now create two distinct slots.
#
# Documented divergence NOT exercised here: bare constant reads
# inside a module body still resolve from the top-level table,
# not the enclosing scope (CRuby's cref walk is a separate Tier 1
# follow-up). Scripts here access nested classes via the
# qualified path (`Foo::Bar`) to stay on the parity-checked path.

# Top-level Bar — method `hello` returns "top".
class Bar
  def hello; "top"; end
end

# Nested Foo::Bar — separate class, separate method.
module Foo
  class Bar
    def hello; "Foo::Bar"; end
  end
end

# Names.
puts Bar.name              # "Bar"
puts Foo::Bar.name         # "Foo::Bar"

# Identity — two distinct Class objects.
puts Bar.equal?(Foo::Bar)  # false

# Independent method tables.
puts Bar.new.hello         # "top"
puts Foo::Bar.new.hello    # "Foo::Bar"

# Re-opening within the SAME scope still hits the same slot.
class Bar
  def kind; "top-kind"; end
end
puts Bar.new.kind          # "top-kind" — same class

module Foo
  class Bar
    def kind; "foo-kind"; end
  end
end
puts Foo::Bar.new.kind     # "foo-kind" — same nested class

# The top-level Bar must NOT have inherited `:foo-kind`.
puts Bar.new.respond_to?(:kind)  # true (own method)
puts Bar.new.kind                # "top-kind" — not foo-kind

# Multi-segment path.
module A
  module B
    class C
      def deep; "A::B::C" end
    end
  end
end
puts A::B::C.name          # "A::B::C"
puts A::B::C.new.deep      # "A::B::C"

# Sibling classes inside the same module have distinct names
# and method tables.
module Sib
  class X
    def who; "X" end
  end
  class Y
    def who; "Y" end
  end
end
puts Sib::X.name           # "Sib::X"
puts Sib::Y.name           # "Sib::Y"
puts Sib::X.equal?(Sib::Y) # false
puts Sib::X.new.who        # "X"
puts Sib::Y.new.who        # "Y"
