# Two related VM fixes locked together:
#
#   1. `Class.new { ... }` block-form — runs the block as class
#      body (`as_class_body=true`), so `def` inside lands on the
#      new class's instance-method table. The new class's
#      superclass defaults to Object; an explicit Class arg
#      overrides (`Class.new(Parent) { ... }`).
#
#   2. `class Sub < <expr>` with arbitrary parent expression —
#      AST translator now accepts any SExpr for the parent
#      (was constant-name-only), and the compiler compiles it
#      to push a Value::Class for `Op::DefClass` to pop. Lets
#      `class Sub < local_var` and
#      `class Sub < DelegateClass(Hash)` work the same way
#      they do in CRuby.
#
# Both surfaced while debugging sinatra-flash's
# `class FlashHash < DelegateClass(Hash)` shape — the gem
# couldn't construct even a baseline FlashHash before either
# fix.

# Class.new with block — `def initialize` lands.
c = Class.new do
  def hello; "hi"; end
end
puts c.new.hello

# `c.class` is Class; the new class is a real Class, not a bare
# Instance pretending to be one.
puts c.class

# Class.new with explicit superclass arg.
class TheParent
  def greet(x); "parent_greet:#{x}"; end
end
c2 = Class.new(TheParent) do
  def shout(x); greet(x).upcase; end
end
puts c2.new.shout("hi")

# Block receives the new class as its sole positional arg
# (CRuby parity: `Class.new { |k| ... }`).
seen_kls = nil
c3 = Class.new do |k|
  seen_kls = k
end
puts c3.equal?(seen_kls)

# `class Sub < local_var` resolves the local var as the parent.
parent_local = TheParent
class Sub < parent_local
  def from_sub; greet("via_sub"); end
end
puts Sub.new.from_sub
puts Sub.superclass.equal?(parent_local)

# `class Sub < method_call(args)` — DelegateClass-shape.
def make_parent_via_method
  Class.new do
    def initialize(x); @x = x; end
    def get_x; @x; end
  end
end
class SubFromMethod < make_parent_via_method
  def initialize(y)
    super(y * 10)
  end
end
puts SubFromMethod.new(5).get_x

# super(*args) inside such a Sub still resolves through the
# anonymous parent's `initialize`. Was the load-bearing case
# for the sinatra-flash FlashHash#initialize shape:
#   `class FlashHash < DelegateClass(Hash); def initialize(s);
#    @now=s; super(@now); end; end`
class Wrapper < Class.new { def initialize(x); @inner = x; end; attr_reader :inner }
  def initialize(y)
    super({outer: y})
  end
end
puts Wrapper.new(42).inner.inspect
