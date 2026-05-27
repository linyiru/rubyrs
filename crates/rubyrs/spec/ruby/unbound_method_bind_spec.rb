# Adapted from ruby/spec core/unboundmethod/bind_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — four
# it-blocks are inlined: returns a Method bound to the receiver;
# the bound Method runs the original body; binds onto a subclass
# instance; raises TypeError on an incompatible receiver.

describe "UnboundMethod#bind" do
  it "returns a Method bound to the given receiver" do
    class UBindT1
      def add(x); x + 1; end
    end
    bound = UBindT1.instance_method(:add).bind(UBindT1.new)
    assert_eq(bound.class.to_s, "Method")
  end

  it "produces a callable Method that runs the original body" do
    class UBindT2
      def add(x); x + 1; end
    end
    u = UBindT2.instance_method(:add)
    assert_eq(u.bind(UBindT2.new).call(5), 6)
  end

  it "binds onto a subclass instance" do
    class UBindT3Base
      def f; :base; end
    end
    class UBindT3Sub < UBindT3Base
    end
    u = UBindT3Base.instance_method(:f)
    assert_eq(u.bind(UBindT3Sub.new).call, :base)
  end

  it "raises TypeError when receiver is incompatible" do
    class UBindT4Lhs
      def f; :ok; end
    end
    class UBindT4Rhs
    end
    u = UBindT4Lhs.instance_method(:f)
    assert_raises("TypeError") { u.bind(UBindT4Rhs.new) }
  end

  # skipped (method-not-implemented): describe "UnboundMethod#bind_call" do ... end
  #   `UnboundMethod#bind_call` is not in the subset; calling
  #   `u.bind_call(recv, *args)` raises NoMethodError. Tracked
  #   for a future batch alongside the `Method#bind_call` gap.
end
