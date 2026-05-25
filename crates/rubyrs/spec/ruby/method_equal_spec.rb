# Adapted from ruby/spec core/method/equal_value_spec.rb +
# shared/eql.rb at 2026-05 (subset). The upstream test
# delegates via `it_behaves_like :method_equal, :==` to
# shared/eql.rb; the relevant cases are inlined here.
#
# Skipped (rubyrs divergence — documented):
#   - "returns true on aliased methods" — rubyrs's
#     `Method#==` compares both the underlying Method pointer
#     and the call name; an `alias_method :baz, :bar` produces
#     a Method whose call-name is `:baz`, so the equality
#     returns false where CRuby's looks through the alias.
#     See `docs/SUBSET.md` → "Method objects" for the
#     concrete divergence example. The skip un-resolves when
#     a future PR aligns Method identity with CRuby.
#   - "returns true if the two core methods are aliases"
#     (`String#size` vs `String#length`) — same root cause as
#     above plus depends on the core method coalescing that
#     primitive_call currently doesn't expose.
#
# Skipped (out of subset):
#   - MethodSpecs::Methods + MethodSpecs::A fixtures (inline
#     defs used instead per `it`)
#   - `before :each` setup
#   - `send(@method, ...)` dispatch through shared examples
#     (we call `==` directly)

describe "Method#==" do
  it "returns true when the same Method is compared to itself" do
    class EqTarget1
      def m; 1; end
    end
    e = EqTarget1.new
    m = e.method(:m)
    assert_eq(m.==(m), true)
  end

  it "returns true for two captures of the same method on the same object" do
    class EqTarget2
      def m; 1; end
    end
    e = EqTarget2.new
    m_a = e.method(:m)
    m_b = e.method(:m)
    assert_eq(m_a.==(m_b), true)
  end

  it "returns false for distinct methods on the same object" do
    class EqTarget3
      def m1; 1; end
      def m2; 2; end
    end
    e = EqTarget3.new
    a = e.method(:m1)
    b = e.method(:m2)
    assert_eq(a.==(b), false)
  end

  it "returns false for the same method bound to different receivers" do
    class EqTarget4
      def m; 1; end
    end
    a = EqTarget4.new
    b = EqTarget4.new
    assert_eq(a.method(:m).==(b.method(:m)), false)
  end

  it "returns false for methods bound to the same object but defined separately" do
    class EqTarget5
      def m1; 1; end
      def m2; 1; end  # same body, different name
    end
    e = EqTarget5.new
    assert_eq(e.method(:m1).==(e.method(:m2)), false)
  end
end
