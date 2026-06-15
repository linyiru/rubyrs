# `Kernel.method_defined?` must answer honestly, not blanket-true. The
# `alias_method … unless method_defined?(:x)` guard idiom (zeitwerk's
# require wrapper, bundler, shims) breaks if an undefined name reports
# defined.
p Kernel.method_defined?(:totally_made_up_xyz)   # false
p Kernel.method_defined?(:another_fake_one)      # false
# `require` / `puts` are private on Kernel → method_defined? is false.
p Kernel.method_defined?(:require)               # false
# A genuine public reflectable builtin is true.
p Kernel.method_defined?(:class)                 # true
# A user-defined public Kernel method reports true.
module Kernel
  def my_public_kernel_method; end
end
p Kernel.method_defined?(:my_public_kernel_method)  # true
# The guard idiom now behaves: method_defined? is false for the
# not-yet-aliased name, so the `unless` guard lets the alias run.
p Kernel.method_defined?(:__probe_orig)          # false (guard fires)
