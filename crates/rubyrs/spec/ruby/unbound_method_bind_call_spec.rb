# Adapted from ruby/spec core/unboundmethod/bind_call_spec.rb
# at upstream commit 448cb340 (2026-05). Hand-translated —
# mirror image of method_bind_call_spec.rb, but the source is
# `Class.instance_method` (UnboundMethod) rather than
# `instance.method`.

describe "UnboundMethod#bind_call" do
  it "invokes the captured method on the given receiver" do
    class UBC1
      def add(x); x + 1; end
    end
    assert_eq(UBC1.instance_method(:add).bind_call(UBC1.new, 5), 6)
  end

  it "binds onto a subclass instance" do
    class UBC2Base
      def f; self.class.to_s; end
    end
    class UBC2Sub < UBC2Base
    end
    assert_eq(UBC2Base.instance_method(:f).bind_call(UBC2Sub.new), "UBC2Sub")
  end

  it "raises TypeError when receiver is incompatible" do
    class UBC3Lhs
      def f; end
    end
    class UBC3Rhs
    end
    u = UBC3Lhs.instance_method(:f)
    assert_raises("TypeError") { u.bind_call(UBC3Rhs.new) }
  end

  it "raises ArgumentError when no receiver is given" do
    class UBC4
      def f; end
    end
    u = UBC4.instance_method(:f)
    assert_raises("ArgumentError") { u.bind_call }
  end
end
