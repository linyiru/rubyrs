# `class << <expr>; self; end` — the ubiquitous "grab the
# eigenclass of an object" idiom, where the receiver is an
# arbitrary expression (a constant, a local) rather than `self`.
# The construct's value is the receiver's singleton class.
# rack's CommonLogger test mocks `Time.now` exactly this way:
#   mc = class << Time; self; end
#   mc.send :alias_method, :old_now, :now
#   mc.send :define_method, :now do ... end

# constant receiver
mc = class << Time; self; end
p mc.is_a?(Class)
p mc.inspect

# plain-object receiver
obj = Object.new
e = class << obj; self; end
p e.is_a?(Class)
p e.equal?(obj.singleton_class)   # same eigenclass object

# local-variable receiver (side-effect-free, but exercises the
# non-constant non-self path)
arr = [1, 2, 3]
ec = class << arr; self; end
p ec.is_a?(Class)

# reflective mock-and-restore on a class's eigenclass, the
# CommonLogger pattern: alias the native method out, redefine it,
# then undef + alias it back.
meta = class << Time; self; end
meta.send :alias_method, :old_now, :now
meta.send :define_method, :now do
  at(0)
end
p Time.now.to_i                   # 0
meta.send :undef_method, :now
meta.send :alias_method, :now, :old_now
p(Time.now.to_i > 1_000_000_000)  # true — real clock restored

# def inside `class << expr` still installs a singleton method
str_holder = "x"
class << str_holder
  def shout
    "loud"
  end
end
p str_holder.shout
