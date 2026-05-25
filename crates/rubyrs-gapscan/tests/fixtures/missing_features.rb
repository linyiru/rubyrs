# Hand-picked fixture: deliberately exercises Prism node classes
# that rubyrs does NOT support. Used by tests/scan.rs to verify
# the classifier reports them as Missing. Counts matter — adjust
# tests/scan.rs together with this file if you change anything.

class Foo                        # ClassNode (supported)
  CONST = 42                     # ConstantWriteNode (× 1)
  Foo::Other = 1                 # ConstantPathWriteNode (× 1)
  def bar
    $1                           # NumberedReferenceReadNode (× 1)
  end
end
