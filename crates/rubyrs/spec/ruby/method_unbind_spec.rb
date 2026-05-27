# Adapted from ruby/spec core/method/unbind_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — the basic
# "returns an UnboundMethod" + "round-trip via bind preserves
# call semantics" shape is inlined. Mock-based tests + the
# Method-from-Module variant are dropped.

describe "Method#unbind" do
  it "returns an UnboundMethod" do
    class UnbT1
      def f; :ok; end
    end
    u = UnbT1.new.method(:f).unbind
    assert_eq(u.class.to_s, "UnboundMethod")
  end

  it "round-trips via bind on a fresh receiver" do
    class UnbT2
      def add(x); x + 1; end
    end
    u = UnbT2.new.method(:add).unbind
    assert_eq(u.bind(UnbT2.new).call(5), 6)
  end

  it "preserves arity through unbind" do
    class UnbT3
      def f(a, b); end
    end
    assert_eq(UnbT3.new.method(:f).unbind.arity, 2)
  end

  it "preserves the owner Module across unbind" do
    class UnbT4
      def f; end
    end
    assert_eq(UnbT4.new.method(:f).unbind.owner == UnbT4, true)
  end
end
