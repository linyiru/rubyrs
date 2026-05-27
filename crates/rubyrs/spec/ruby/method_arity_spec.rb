# Adapted from ruby/spec core/method/arity_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — six it-blocks
# are inlined: zero, fixed-positional, splat (`-1`), one
# required + one optional (`-(n+1)` shape), block-only
# (arity 0), and "block param does not count toward arity".
# Mock-based shape tests + define_method-form blocks dropped.

describe "Method#arity" do
  it "returns 0 for a method that takes no arguments" do
    class ArityT0
      def f; end
    end
    assert_eq(ArityT0.new.method(:f).arity, 0)
  end

  it "returns the number of arguments for a method with fixed positionals" do
    class ArityT1
      def f(a, b); end
    end
    assert_eq(ArityT1.new.method(:f).arity, 2)
  end

  it "returns -1 for a method with a splat" do
    class ArityT2
      def f(*a); end
    end
    assert_eq(ArityT2.new.method(:f).arity, -1)
  end

  it "returns -(n+1) for a method with n required and any optional positionals" do
    class ArityT3
      def f(a, b = 1); end
    end
    assert_eq(ArityT3.new.method(:f).arity, -2)
  end

  it "returns 0 when the only param is a block param" do
    class ArityT4
      def f(&blk); end
    end
    assert_eq(ArityT4.new.method(:f).arity, 0)
  end

  it "does not count the block param toward arity" do
    class ArityT5
      def f(a, b, &blk); end
    end
    assert_eq(ArityT5.new.method(:f).arity, 2)
  end
end
