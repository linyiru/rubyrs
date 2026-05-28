# Method#to_s / Method#inspect — render
#   `#<Method: RecvClass#name(params)>` (or `RecvClass(DefiningClass)`
#   when foo is inherited)
#   `#<UnboundMethod: DefiningClass#name(params)>`
# matching CRuby's form. CRuby tacks on a ` path:line` source
# location suffix; we don't track per-method location yet, so
# this fixture uses prefix-style assertions to stay diff-parity
# under both interpreters.
#
# Tier-2 follow-up to PR #272 (universal Object#to_s/inspect):
# the universal `#<Class:0xHEX>` form was losing the receiver
# class + method name that defensive logging idioms depend on.

class A; def foo; end; def bar(x, y); end; end
class B < A; def baz; end; end

# BoundMethod, same receiver/defining class
m = A.new.method(:foo)
puts m.inspect.start_with?("#<Method: A#foo()")

# BoundMethod, inherited — receiver class != defining class
m_inh = B.new.method(:foo)
puts m_inh.inspect.start_with?("#<Method: B(A)#foo()")

# BoundMethod with parameter list
m_args = A.new.method(:bar)
puts m_args.inspect.start_with?("#<Method: A#bar(x, y)")

# UnboundMethod uses the defining class, not the capturing one
puts A.instance_method(:foo).inspect.start_with?("#<UnboundMethod: A#foo()")
puts B.instance_method(:foo).inspect.start_with?("#<UnboundMethod: A#foo()")

# to_s and inspect produce the same form (CRuby parity)
puts m.to_s == m.inspect
puts A.instance_method(:foo).to_s == A.instance_method(:foo).inspect

# Round-trip: bound → unbind → bind → bound. Re-bound Method's
# inspect still uses the receiver's actual class.
ubm = A.instance_method(:foo)
rb = ubm.bind(A.new)
puts rb.inspect.start_with?("#<Method: A#foo()")
