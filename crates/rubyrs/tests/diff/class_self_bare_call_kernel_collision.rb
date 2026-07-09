# Ticket 3: a bare call from a class-method body whose name collides
# with a Kernel private function must resolve the class's OWN singleton
# method table BEFORE falling to the Kernel builtin (CRuby method
# lookup runs before Kernel's private builtins). The class-self
# bare-call bucket declined every `is_builtin_name`, so the Kernel
# builtin won.

class H
  def self.format(x); "F:#{x}"; end
  def self.go; format("hi"); end
end
p H.go                       # => "F:hi" (not Kernel#format's "hi")

class P
  def self.print(x); "P:#{x}"; end
  def self.go; print("hi"); end
end
p P.go

class S
  def self.sprintf(x); "SP:#{x}"; end
  def self.go; sprintf("hi"); end
end
p S.go

class I
  def self.Integer(x); "INT:#{x}"; end
  def self.go; Integer("5"); end
end
p I.go

module M
  def self.format(x); "MF:#{x}"; end
  def self.go; format("hi"); end
end
p M.go

# Negative: a bare Kernel call NOT shadowed still reaches Kernel.
class N
  def self.go; puts("reached"); 42; end
end
p N.go                       # prints "reached", then 42

# NOTE: the Object-instance-method collision (`class O; def format;
# …; def go; format(x); end; end`) is deliberately NOT asserted here.
# The interpreter already resolves it correctly (via
# try_invoke_self_recv_cached, unchanged by this fix), but the
# native JIT has a SEPARATE pre-existing gap — a compiled body's bare
# call to a Kernel-colliding name resolves to the builtin — so pinning
# it here would red the jit-native diff leg on an unrelated bug.
