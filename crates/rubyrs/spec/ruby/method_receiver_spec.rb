# Adapted from ruby/spec core/method/receiver_spec.rb at
# 2026-05 (subset). Skipped:
#   - MethodSpecs::* fixtures (inline classes per `it` block)
#   - respond_to_missing?-generated Method dispatch (separate
#     spec area)

describe "Method#receiver" do
  it "returns the object the Method was bound to" do
    class RecT1
      def m; 1; end
    end
    obj = RecT1.new
    # Upstream uses `should.equal?(s)` (identity). assert_eq
    # against the value of `equal?(...)` to express the same.
    assert_eq(obj.method(:m).receiver.equal?(obj), true)
  end

  it "returns the same receiver for the original and an alias" do
    class RecT2
      def foo; "f"; end
      alias_method :bar, :foo
    end
    obj = RecT2.new
    assert_eq(obj.method(:foo).receiver.equal?(obj), true)
    assert_eq(obj.method(:bar).receiver.equal?(obj), true)
  end

  it "returns the object even when the method was inherited" do
    class RecT3_Parent
      def hello; "p"; end
    end
    class RecT3_Child < RecT3_Parent
    end
    child = RecT3_Child.new
    assert_eq(child.method(:hello).receiver.equal?(child), true)
  end

  it "distinguishes receivers across two instances of the same class" do
    class RecT4
      def m; nil; end
    end
    a = RecT4.new
    b = RecT4.new
    assert_eq(a.method(:m).receiver.equal?(a), true)
    assert_eq(a.method(:m).receiver.equal?(b), false)
  end
end
