# Kernel#binding captures the calling scope's self; eval(src, binding)
# runs the source with that self, so method calls + ivar reads in the
# eval'd string dispatch against it. Self-dispatch layer (rack's
# Builder.new_from_string evals a rackup script against
# `builder.instance_eval { binding }`). Zero require — covers the
# native binding builtin + the eval(src, binding) arms in vm/kernel.rs.
# (Outer local-variable capture is a follow-up, not exercised here.)
class Ctx
  def initialize(n); @n = n; end
  def double(x); x * 2; end
  def label; "ctx#{@n}"; end
  def grab; binding; end
  def grab_ie; instance_eval { binding }; end
end

c = Ctx.new(5)
b1 = c.grab
puts b1.class                          # Binding
puts eval("double(21)", b1)            # 42  (self = c)
puts eval("label", b1)                 # ctx5
puts eval("double(double(3))", b1)     # 12
puts eval("@n", b1)                    # 5   (ivar via captured self)
puts eval("label", b1, "(rackup)")     # ctx5 (3-arg: src, binding, file)

# instance_eval-captured binding (the rack Builder shape).
b2 = c.grab_ie
puts eval("double(10)", b2)            # 20

# eval with no binding still works; a non-Binding 2nd arg is dropped.
puts eval("1 + 2")                     # 3
puts eval("3 * 3", nil)                # 9

# A fresh instance does NOT share the captured self.
d = Ctx.new(9)
puts eval("label", d.grab)             # ctx9
