# `Module#private_constant` and `Module#public_constant` — stubs
# that accept Symbol / String args and return the receiver. CRuby
# tags the named constants so external access raises NameError;
# rubyrs accepts the call (so tilt-style class bodies load) but
# does NOT model enforcement yet. Internal references work in
# both runtimes; external references that CRuby would reject are
# permitted by rubyrs — documented divergence. The fixture
# exercises only the call shape and internal-access path, which
# is the part both runtimes agree on.

module Foo
  HELLO = "hi"
  WORLD = "world"
  private_constant :HELLO
  public_constant :WORLD

  # Internal access — both runtimes resolve.
  def self.greet
    HELLO + ", " + WORLD
  end
end

puts Foo.greet                       # "hi, world"
puts Foo::WORLD                      # "world" (public)

# String-arg form is also accepted in CRuby. rubyrs's stub
# accepts both.
class Bar
  ANSWER = 42
  private_constant "ANSWER"
end
# Internal access via a class method.
class Bar
  def self.answer; ANSWER; end
end
puts Bar.answer                       # 42

# Variadic — multiple constants at once.
module Many
  A = 1
  B = 2
  C = 3
  private_constant :A, :B, :C
  def self.all
    [A, B, C]
  end
end
p Many.all                            # [1, 2, 3]

# Return value is the receiver in CRuby; verify both runtimes
# agree the call doesn't raise and the script keeps running.
module Ret
  X = 10
  Y = private_constant(:X)
end
puts Ret::Y == Ret                    # true

# 0-arg form is a no-op returning the receiver in CRuby — locks
# that the stub doesn't trip its arg-shape guard on the empty
# call.
class Empty
  Z = public_constant
end
puts Empty::Z == Empty                # true
