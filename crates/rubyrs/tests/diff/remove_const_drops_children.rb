# Removing a namespace constant makes its NESTED constants unreachable too
# (CRuby). rubyrs stores `Ns::Child` as a flat key, so remove_const must also
# drop every `Ns::*` descendant — else a recreated Ns still sees stale children.
# zeitwerk's reload teardown does `remove_const :Ns` between tests, then reloads.
module Ns
  Bar = 1
  class Inner; end
  module Deep; Leaf = 2; end
end
p Ns::Bar
p Ns::Deep::Leaf
Object.send(:remove_const, :Ns)

module Ns; end                          # recreate fresh
p Ns.const_defined?(:Bar, false)        # false — Bar gone with old Ns
p Ns.const_defined?(:Inner, false)      # false
p Object.const_defined?("Ns::Bar")      # false
p Object.const_defined?("Ns::Deep")     # false
p Object.const_defined?("Ns::Deep::Leaf") # false
