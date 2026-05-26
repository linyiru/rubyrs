# Class#name / #to_s / #inspect return the fully-qualified
# lexical path when a class is defined inside a module/class
# body. Pre-fix rubyrs returned the bare local name only
# (`Buffer` instead of `MessagePack::Buffer`), diverging from
# CRuby and the existing PR #89 nested-module dual-write
# direction.
#
# Surfaced while writing `cext_msgpack_pure_ruby_load.rb` —
# the four `MessagePack::*` cext class shells all wanted to
# report their qualified name via `.name`. The fixture there
# avoids the `.name` path; this fixture covers it directly.
#
# Edge cases NOT in scope:
#   - Class re-open in a different scope: `class Foo; end`
#     at top level followed by `module M; class Foo; ...; end;
#     end` keeps the first-define's name ("Foo"). The class
#     name is stamped on initial creation only — re-opens go
#     through the `or_insert_with` short-circuit. This is
#     consistent with the existing SUBSET "same bare name
#     collides at top-level" entry under PR #89.

# Top-level class: bare name unchanged.
class TopLevel
end
puts TopLevel.name           # "TopLevel"
puts TopLevel.to_s
puts TopLevel.inspect

# 2-level nesting.
module Foo
  class Bar
  end
end
puts Foo::Bar.name           # "Foo::Bar"
puts Foo::Bar.to_s
puts Foo::Bar.inspect

# 3-level nesting.
module A
  module B
    class C
    end
  end
end
puts A::B::C.name            # "A::B::C"

# Class inside class (not just module).
class Outer
  class Inner
  end
end
puts Outer::Inner.name       # "Outer::Inner"

# Module-inside-module-inside-class.
class Wrapper
  module Helper
    class Job
    end
  end
end
puts Wrapper::Helper::Job.name  # "Wrapper::Helper::Job"

# Class inside top-level module — the msgpack-ruby
# `class MessagePack::Buffer` shape.
module Msg
  class Buffer
  end
  class Packer
  end
  class Unpacker
  end
end
puts Msg::Buffer.name        # "Msg::Buffer"
puts Msg::Packer.name        # "Msg::Packer"
puts Msg::Unpacker.name      # "Msg::Unpacker"

# Identity invariants survive — qualified name is purely a
# display change.
puts Foo::Bar.equal?(Foo::Bar)   # true
puts Foo::Bar.is_a?(Class)        # true
