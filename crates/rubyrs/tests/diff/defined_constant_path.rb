# `defined?(Foo::Bar)` — qualified-path defined? lookup. Prior to
# this commit the AST `defined?` arm only matched ConstantReadNode
# and fell through to `"expression"` for any ConstantPathNode, so
# `defined?(Foo::Bar)` always returned `"expression"` even when
# `Foo::Bar` was a real constant.
#
# Fix has two parts:
#   - AST: ConstantPath inside defined? flattens to the joined
#     name and routes through `__defined_const?` (same plumbing
#     ConstantReadNode already used).
#   - Runtime: `__defined_const?` now also consults
#     `self.constants` (was only `self.classes`), so user-
#     assigned `Foo::Bar = 1` reports correctly. Mirrors the
#     fallback chain `Op::LoadConst` walks.

module Foo; end
Foo::Bar = 42
puts defined?(Foo::Bar).inspect              # "constant"
puts defined?(Foo).inspect                   # "constant" (the module shell)
puts defined?(Foo::Missing).inspect          # nil → blank line

::Top = 1
puts defined?(::Top).inspect                 # "constant"

module Outer
  module Inner
    Const = "hi"
  end
end
puts defined?(Outer::Inner).inspect          # "constant"
puts defined?(Outer::Inner::Const).inspect   # "constant"

# Bare constant reads still report correctly — regression check.
TopLevel = 9
puts defined?(TopLevel).inspect              # "constant"
puts defined?(NeverDefined).inspect          # nil
