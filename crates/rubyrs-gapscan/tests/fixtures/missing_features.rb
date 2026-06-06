# Hand-picked fixture: deliberately exercises Prism node classes
# that rubyrs does NOT support. Used by tests/scan.rs to verify
# the classifier reports them as Missing. Counts matter — adjust
# tests/scan.rs together with this file if you change anything.
#
# build.rs's fixture-drift guard validates these annotations stay
# in-sync with rubyrs's actual Missing set; when a feature lands
# and closes one of these gaps, the build fails with a list of
# still-Missing replacement candidates.

class Foo                        # ClassNode (supported)
  def bar
    1i                           # ImaginaryNode (× 1)
  end
  def quux
    $&                           # BackReferenceReadNode (× 1)
  end
  alias $new_global $old_global  # AliasGlobalVariableNode (× 1)
end
