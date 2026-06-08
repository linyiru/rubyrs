# Kernel's methods are `module_function`s: callable BOTH as a
# bare private instance method (implicit self) AND as a public
# method on the Kernel module object itself (`Kernel.foo` /
# `Kernel::foo`). rouge.rb:43 does `Kernel::load File.join(...)`.
#
# Pre-fix: an explicit-receiver call whose receiver is the
# Kernel module ("undefined method 'load' for Class") fell
# through to NoMethodError because Kernel builtins were only
# routed for the bare form; `Kernel.respond_to?(:load)` was
# also false. Now a Kernel-module receiver routes through the
# same `builtin_call` the bare form uses.

# 1. respond_to? agrees with what dispatch accepts.
p Kernel.respond_to?(:load)
p Kernel.respond_to?(:require)
p Kernel.respond_to?(:puts)
p Kernel.respond_to?(:print)
p Kernel.respond_to?(:eval)
p Kernel.respond_to?(:format)
p Kernel.respond_to?(:Integer)
p Kernel.respond_to?(:totally_made_up_name)

# 2. `Kernel.load` of a missing file runs the builtin → LoadError
#    (NOT NoMethodError). LoadError < ScriptError, so it is NOT a
#    StandardError — `rescue LoadError` is required to catch it.
begin
  Kernel.load("/nonexistent_rubyrs_xyz.rb")
rescue LoadError => e
  puts e.class
end

# 3. The `Kernel::load` (colon-colon) call shape behaves
#    identically — same dispatch as `Kernel.load`.
begin
  Kernel::load("/nonexistent_rubyrs_xyz.rb")
rescue LoadError => e
  puts e.class
end

# 4. `Kernel.require` of a missing lib → LoadError too.
begin
  Kernel.require("no_such_rubyrs_library_zzz")
rescue LoadError => e
  puts e.class
end

# 5. Other Kernel module functions called as `Kernel.foo`.
Kernel.puts("via Kernel.puts")
Kernel.print("via Kernel.print\n")
p Kernel.format("%04d:%s", 7, "ok")
p Kernel.sprintf("%x", 255)
p Kernel.Integer("123")
p Kernel.Integer("0xff", 16)
p Kernel.Float("3.5")
p Kernel.String(42)
p Kernel.Array(nil)
p Kernel.Array([1, 2])

# 6. An undefined name on Kernel is still a NoMethodError.
begin
  Kernel.this_is_not_a_kernel_method
rescue NoMethodError
  puts "NoMethodError"
end
