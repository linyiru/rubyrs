# Lexical constant nesting — `module Foo; class Bar; end; end`
# now stores under BOTH `Bar` (bare, for inside-scope reads) and
# `Foo::Bar` (prefixed, for outside path-reads). Dual-write keeps
# both lookup directions working without modelling CRuby's full
# cref-walk constant lookup.

# --- Basic 2-level module + class ---
module Tilt
  class StringTemplate
    def hello; "hello from string template"; end
  end
  VERSION = "1.0"
end

puts Tilt::StringTemplate.new.hello
puts Tilt::VERSION

# --- 3-level nesting ---
module A
  module B
    class C
      def deep; "deep value"; end
    end
    INNER = 42
  end
end

puts A::B::C.new.deep
puts A::B::INNER

# --- Bare read from inside scope (the inside-direction half of
# the dual-write) ---
module Lib
  X = 100
  Y = X + 1               # bare X read finds 100
  class Reader
    def get_x; X; end     # method body still finds bare X
  end
end

puts Lib::X
puts Lib::Y
puts Lib::Reader.new.get_x

# --- Class inside class (not just module) ---
class Outer
  class Inner
    def whoami; "inner"; end
  end
end

puts Outer::Inner.new.whoami

# --- Path-form write inside scope (already worked pre-PR;
# regression guard) ---
module Cfg
  Defaults = {name: "spike"}
end
puts Cfg::Defaults[:name]

# --- Absolute paths (`::X = ...`) skip the lexical alias ---
# Inside `module Wrapper`, `::TOP_ABS = ...` should store ONLY
# at top-level — `Wrapper::TOP_ABS` must NOT be created. We
# verify by checking the top-level value round-trips.
module Wrapper
  ::TOP_ABS = "from absolute write"
end
puts TOP_ABS

# Same for the `::Foo::Bar = ...` form inside a nested scope —
# leading `::` keeps the write top-level-rooted.
module Outer2
  class Inner2
    ::FromInner = "deep absolute"
  end
end
puts FromInner

# --- Reopening preserves the alias (DefClass is idempotent) ---
module Reopen
  class Box
    def first; 1; end
  end
end
module Reopen
  class Box
    def second; 2; end
  end
end
b = Reopen::Box.new
puts b.first
puts b.second
