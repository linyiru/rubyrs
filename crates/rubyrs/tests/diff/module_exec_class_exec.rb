# Module#module_exec / Module#class_exec — the block-with-args twin of
# class_eval's block form: runs the block in the receiver class's body
# context (define_method/def land on the class) but the block receives
# the EXPLICIT args passed, not the class itself. rspec builds example
# groups via `klass.module_exec(*args, &block)` on an anonymous subclass.

k = Class.new
k.module_exec(1, 2) { |a, b| define_method(:sum) { a + b } }
p k.new.sum                      # 3

# class_exec is the same method under its other name.
k2 = Class.new
k2.class_exec(5) { |a| define_method(:five) { a } }
p k2.new.five                    # 5

# self inside the block is the class (def lands on it).
k3 = Class.new
k3.class_exec do
  def hello; "hi"; end
end
p k3.new.hello                   # "hi"

# Args flow through; no args → block gets none.
k4 = Class.new
captured = nil
k4.module_exec("x", "y", "z") { |*a| captured = a }
p captured                       # ["x", "y", "z"]

# Works on a named class and mutates it.
class Widget; end
Widget.class_exec(42) { |n| define_method(:answer) { n } }
p Widget.new.answer              # 42

# Modules too (it's a Module instance method).
m = Module.new
m.module_exec { def shared; :shared; end }
c = Class.new { include m }
p c.new.shared                   # :shared

# (The block form's return value — `Class.new.class_exec(10) { |n| n*3 }`
# — is a documented divergence shared with class_eval's block form:
# rubyrs returns the class, CRuby returns the block value. Omitted here
# since both share the same `invoke_block_with_self(as_class_body)` frame
# as `Class.new { }`, which MUST return the class.)
