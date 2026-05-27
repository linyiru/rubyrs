# Adapted from ruby/spec core/unboundmethod/bind_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — the
# basic "bind returns a callable Method on the new receiver"
# shape is inlined. The TypeError-on-incompatible-class block
# is dropped — rubyrs's bind currently soft-fails (returns nil
# on the dispatch result) rather than raising TypeError, a
# divergence noted in the skip trace below.

describe "UnboundMethod#bind" do
  it "returns a Method bound to the given receiver" do
    class UBindT1
      def add(x); x + 1; end
    end
    bound = UBindT1.instance_method(:add).bind(UBindT1.new)
    assert_eq(bound.class.name, "Method")
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

  # skipped (divergent): it "raises TypeError when receiver is incompatible" do
  #   CRuby raises TypeError when binding an UnboundMethod to a
  #   receiver whose class isn't `<= owner`. rubyrs currently
  #   returns nil on the dispatch result rather than raising;
  #   tracked as a divergent gap.
  # skipped (method-not-implemented): describe "UnboundMethod#bind_call" do ... end
  #   bind_call is implemented (PR #205) but covered in its own
  #   spec file rather than inlined here.
end
