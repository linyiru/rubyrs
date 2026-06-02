# Method#super_method / UnboundMethod#super_method — returns the
# Method that `super` would dispatch to, or nil if no super
# definition exists. Walks past the captured Method's defining
# class and resolves the same name against the ancestor chain.

class A
  def foo; "A.foo"; end
  def bar; "A.bar"; end
end

class B < A
  def foo; "B.foo+#{super}"; end
end

class C < B
  def foo; "C.foo+#{super}"; end
end

# (1) Bound — walks one step at a time.
m_c = C.new.method(:foo)
puts m_c.name
puts m_c.owner.name           # C
sm = m_c.super_method
puts sm.name                  # foo
puts sm.owner.name            # B
puts sm.call                  # B.foo+A.foo
ssm = sm.super_method
puts ssm.owner.name           # A
puts ssm.call                 # A.foo

# (2) Super chain terminates at root (A's foo has no further super).
puts ssm.super_method.nil?    # true

# (3) A.foo bound directly — no super (Object doesn't define foo).
puts A.new.method(:foo).super_method.nil?  # true

# (4) Method without super behaves the same on bar.
puts C.new.method(:bar).owner.name              # A
puts C.new.method(:bar).super_method.nil?       # true

# (5) UnboundMethod — same shape, anchored on the super-defining class.
u_c = C.instance_method(:foo)
puts u_c.owner.name           # C
us = u_c.super_method
puts us.name                  # foo
puts us.owner.name            # B
us2 = us.super_method
puts us2.owner.name           # A
puts us2.super_method.nil?    # true

# (6) Calling super_method on a Bound BoundMethod preserves recv —
# the returned BoundMethod is still bound to the original receiver,
# so calling it invokes B#foo on a C instance.
c = C.new
b_super = c.method(:foo).super_method
puts b_super.receiver.equal?(c)   # true
puts b_super.call                 # B.foo+A.foo

# (7) Wrong arity → ArgumentError.
begin
  C.new.method(:foo).super_method(1)
rescue ArgumentError => e
  puts e.message
end

# (8) respond_to?
puts C.new.method(:foo).respond_to?(:super_method)
puts C.instance_method(:foo).respond_to?(:super_method)
