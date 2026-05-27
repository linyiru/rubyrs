# Adapted from ruby/spec core/method/parameters_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — eight
# it-blocks are inlined covering all parameter shapes:
# empty, req, opt, rest, key/keyreq, keyrest, block (basic),
# and a full-mix ordering check (block trails everything).
# The define_method-form variant is dropped.

describe "Method#parameters" do
  it "returns an empty array for a no-argument method" do
    class ParamsT0
      def f; end
    end
    assert_eq(ParamsT0.new.method(:f).parameters, [])
  end

  it "reports required positionals as [:req, name]" do
    class ParamsT1
      def f(a, b); end
    end
    assert_eq(ParamsT1.new.method(:f).parameters, [[:req, :a], [:req, :b]])
  end

  it "reports optional positionals as [:opt, name]" do
    class ParamsT2
      def f(a, b = 1); end
    end
    assert_eq(ParamsT2.new.method(:f).parameters, [[:req, :a], [:opt, :b]])
  end

  it "reports a splat as [:rest, name]" do
    class ParamsT3
      def f(*a); end
    end
    assert_eq(ParamsT3.new.method(:f).parameters, [[:rest, :a]])
  end

  it "reports keyword args as [:keyreq, name] / [:key, name]" do
    class ParamsT4
      def f(a:, b: 1); end
    end
    assert_eq(ParamsT4.new.method(:f).parameters, [[:keyreq, :a], [:key, :b]])
  end

  it "reports a double-splat as [:keyrest, name]" do
    class ParamsT5
      def f(**opts); end
    end
    assert_eq(ParamsT5.new.method(:f).parameters, [[:keyrest, :opts]])
  end

  it "reports a block param as [:block, name]" do
    class ParamsT6
      def f(&blk); end
    end
    assert_eq(ParamsT6.new.method(:f).parameters, [[:block, :blk]])
  end

  it "places the block param after positional / keyword params" do
    class ParamsT7
      def f(a, b = 1, *rest, k:, kk: 1, **kr, &blk); end
    end
    assert_eq(
      ParamsT7.new.method(:f).parameters,
      [[:req, :a], [:opt, :b], [:rest, :rest], [:keyreq, :k], [:key, :kk], [:keyrest, :kr], [:block, :blk]],
    )
  end

  # skipped (method-not-implemented): describe "for define_method blocks" do ... end
end
