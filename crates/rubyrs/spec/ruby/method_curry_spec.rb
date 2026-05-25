# Adapted from ruby/spec core/method/curry_spec.rb at 2026-05
# (subset). Skipped:
#   - `before(:each)` setup (each `it` builds its own
#     receiver inline)
#   - MethodSpecs::Methods fixture file methods (`zero`,
#     `one_req`, `one_req_one_opt`, `zero_with_splat`,
#     `two_req_with_splat`, `two_req_one_opt_with_block` —
#     covered loosely here via inline def's where applicable)
#   - ArgumentError on excess curry-arity (rubyrs's `Method#curry`
#     accepts an int arity arg; bound/error behaviour pending
#     dedicated spec)
#   - Singleton-method curry (`def x.foo`)

describe "Method#curry" do
  it "returns a curried Proc" do
    class CurryTarget1
      def add3(a, b, c); a + b + c; end
    end
    c = CurryTarget1.new.method(:add3).curry
    assert_eq(c.is_a?(Proc), true)
  end

  it "supports chained partial application" do
    class CurryTarget2
      def triple(a, b, c); [a, b, c]; end
    end
    c = CurryTarget2.new.method(:triple).curry
    # Apply one arg at a time, then finalise.
    assert_eq(c.call(1).call(2).call(3), [1, 2, 3])
  end

  it "supports applying multiple args at once before final call" do
    class CurryTarget3
      def four(a, b, c, d); a + b + c + d; end
    end
    c = CurryTarget3.new.method(:four).curry
    # Partial application can take more than one arg per step.
    assert_eq(c.call(1, 2).call(3, 4), 10)
  end

  it "preserves the bound receiver across partial application" do
    class CurryTarget4
      def initialize(base); @base = base; end
      def shift(a, b); a + b + @base; end
    end
    t = CurryTarget4.new(100)
    c = t.method(:shift).curry
    assert_eq(c.call(2).call(3), 105)
  end

  describe "with explicit arity argument" do
    it "returns a curried Proc when the arity matches" do
      class CurryTarget5
        def add3(a, b, c); a + b + c; end
      end
      c = CurryTarget5.new.method(:add3).curry(3)
      assert_eq(c.is_a?(Proc), true)
      assert_eq(c.call(1).call(2).call(3), 6)
    end
  end
end
