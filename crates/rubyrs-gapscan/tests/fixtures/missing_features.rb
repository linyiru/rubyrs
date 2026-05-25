# Hand-picked fixture: deliberately exercises Prism node classes
# that rubyrs does NOT support. Used by tests/scan.rs to verify
# the classifier reports them as Missing. Counts matter — adjust
# tests/scan.rs together with this file if you change anything.

module Foo                       # ModuleNode (× 1)
  CONST = 42                     # ConstantWriteNode (× 1)
  def bar
    case CONST                   # CaseNode (× 1)
    when 0
      "zero"
    else
      "non-zero"
    end
  end
end
