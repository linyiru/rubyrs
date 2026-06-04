# `Klass.extend(M)` — CRuby-equivalent to
# `class << Klass; include M; end`: M's instance methods become
# CLASS-level methods of Klass, NOT instance methods of Klass.
# Surfaced by sinatra-contrib/MultiRoute and other plugins that
# `register MyPlugin` against the app class — `register` is
# typically backed by `extend`.

# Baseline: M's methods are class-level on K, NOT instance-level.
module M
  def foo; "M_foo"; end
  def bar; "M_bar"; end
end

class K; end
K.extend(M)
puts K.foo
puts K.bar
puts (begin K.new.foo; rescue NoMethodError => e; "no foo on instance"; end)

# `extend` inside a class body — same shape, but the call is
# bareword (no_recv) with self=Klass.
class L
  extend M
end
puts L.foo
puts L.bar

# Inheritance: K2 < K1, K1.extend(M) — K2 sees M's methods via
# the singleton ancestor walk (singleton_methods miss → next
# superclass step's extended modules).
class K1; end
K1.extend(M)
class K2 < K1; end
puts K2.foo

# Class-method `super` resolves THROUGH an extended module —
# M#hello sits between Klass's own class methods and Klass's
# superclass's class methods. `super` from inside M#hello reaches
# the superclass's `def self.hello`.
module Mod2
  def greet(*args)
    "Mod2[#{args.inspect}]->" + super(*args.map(&:upcase))
  end
end
class Parent
  def self.greet(*args); "Parent[#{args.inspect}]"; end
end
class Child < Parent
  extend Mod2
end
puts Child.greet("hi", "lo")
