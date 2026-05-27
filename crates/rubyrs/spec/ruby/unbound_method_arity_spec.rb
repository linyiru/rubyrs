# Adapted from ruby/spec core/unboundmethod/arity_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — same
# four arity shapes as `method_arity_spec.rb`. UnboundMethod
# carries the same underlying method-entry as a bound Method,
# so the arity values match.

describe "UnboundMethod#arity" do
  it "returns 0 for a method that takes no arguments" do
    class UArityT0
      def f; end
    end
    assert_eq(UArityT0.instance_method(:f).arity, 0)
  end

  it "returns the number of arguments for a method with fixed positionals" do
    class UArityT1
      def f(a, b); end
    end
    assert_eq(UArityT1.instance_method(:f).arity, 2)
  end

  it "returns -1 for a method with a splat" do
    class UArityT2
      def f(*a); end
    end
    assert_eq(UArityT2.instance_method(:f).arity, -1)
  end

  it "returns -(n+1) for a method with n required and any optional positionals" do
    class UArityT3
      def f(a, b = 1); end
    end
    assert_eq(UArityT3.instance_method(:f).arity, -2)
  end
end
