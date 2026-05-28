# Method#inspect for singleton methods — render
#   `#<Method: #<RecvClass:0xHEX>.name(params)>`
# with a `.` separator (not `#`) and the receiver's inspect
# form (not the eigenclass name). Matches CRuby.
#
# Tier-2 follow-up to PR #282: the first cut rendered
# `#<Method: A(#<Class:#<A>>)#singleton_foo()>` because the
# defining-class branch decorated with `RecvClass(EigenclassName)`
# instead of recognizing singleton methods as a distinct case.

class A; def regular; end; end

obj = A.new
def obj.sing; end
def obj.sing_args(x, y); end

# Singleton method: `.` separator, receiver-as-inspect-form
sm = obj.method(:sing)
puts sm.inspect.start_with?("#<Method: #<A:0x")
puts sm.inspect.include?(">.sing(")

# Args carry through
sm_args = obj.method(:sing_args)
puts sm_args.inspect.include?(">.sing_args(x, y)")

# Regular methods on the same object still use `#` + class name
# (the singleton-method check must NOT spuriously fire for them).
rm = obj.method(:regular)
puts rm.inspect.start_with?("#<Method: A#regular()")

# A different instance without singletons stays in the regular
# branch — the check shouldn't depend on _any_ object having
# an eigenclass elsewhere.
other = A.new
puts other.method(:regular).inspect.start_with?("#<Method: A#regular()")
