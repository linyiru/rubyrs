# Module vs Class distinction. Pre-fix, rubyrs modelled
# everything as `Class` — `module X; end` and `class X; end`
# produced identical-shape values, so `is_a?(Class)` was
# true for Modules (wrong) and `class_of` always said
# "Class" (wrong for Modules).
#
# This fixture pins the four diverging cases:
#   - `module X; end` produces a Module-kind shell
#   - `class X; end` produces a Class-kind shell
#   - `is_a?(Class)` distinguishes the two
#   - `is_a?(Module)` is true for both (Class < Module)
#   - `class_of` / `.class` returns "Class" vs "Module"
#
# Stdlib stubs use the right kind per name:
#   - URI, JSON, Base64, Forwardable, Singleton,
#     FileUtils, Digest, YAML, SecureRandom, Open3,
#     Shellwords are Modules
#   - Logger, Set, Pathname, Tempfile, StringIO, Date,
#     OpenStruct, Delegator, OptionParser, BigDecimal,
#     Monitor, ERB, WeakRef are Classes
#
# Documented gaps NOT exercised:
#   - rubyrs still uses the same `Class` struct for both;
#     `Module#instance_methods` and other introspection
#     APIs are missing.
#   - `class X` reopening a previously-defined `module X`
#     (or vice versa) keeps the original kind. CRuby
#     raises TypeError; rubyrs leniently keeps the
#     first-defined kind. Fixture stays away from
#     mixed-keyword re-opens.

class Foo
end
module Bar
end

puts Foo.class           # Class
puts Bar.class           # Module
puts Foo.is_a?(Class)    # true
puts Foo.is_a?(Module)   # true  (Class < Module)
puts Bar.is_a?(Class)    # false
puts Bar.is_a?(Module)   # true

# Nested module + class — qualified names + correct kinds.
module Outer
  module Inner
    class Leaf
    end
  end
end
puts Outer.class                    # Module
puts Outer::Inner.class             # Module
puts Outer::Inner::Leaf.class       # Class
puts Outer::Inner::Leaf.is_a?(Module)  # true
puts Outer.is_a?(Class)             # false

# Stdlib stub: Module-shaped names report "Module".
require 'uri'
puts URI.class                      # Module
puts URI.is_a?(Module)              # true
puts URI.is_a?(Class)               # false

# Stdlib stub: Class-shaped names report "Class".
require 'logger'
puts Logger.class                   # Class
puts Logger.is_a?(Class)            # true
puts Logger.is_a?(Module)           # true

# Module/Class identity round-trip.
puts URI.equal?(URI)                # true
puts Logger.equal?(Logger)          # true
