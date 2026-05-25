# Hand-picked fixture: deliberately exercises Prism node classes
# that rubyrs does NOT support. Used by tests/scan.rs to verify
# the classifier reports them as Missing. Counts matter — adjust
# tests/scan.rs together with this file if you change anything.

module Foo                       # ModuleNode (× 1)
  CONST = 42                     # ConstantWriteNode (× 1)
  def bar
    /\A\d+\z/                    # RegularExpressionNode (× 1)
  end
end
