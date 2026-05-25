# Adapted from ruby/spec core/method/owner_spec.rb at 2026-05
# (subset). Skipped:
#   - MethodSpecs::* fixtures (inline classes per `it` block
#     instead)
#   - respond_to_missing?-generated Method dispatch (separate
#     spec area; method_missing_spec covers the surface)
#   - `public` / `private` visibility manipulation in ancestor
#     chain (covered by separate visibility-spec area)

describe "Method#owner" do
  it "returns the class where the method is defined" do
    class OwnT1
      def m; 1; end
    end
    assert_eq(OwnT1.new.method(:m).owner == OwnT1, true)
  end

  it "returns the ancestor class for an inherited method" do
    # CRuby: "returns the class/module it was defined in" — the
    # owner of an inherited Method is the class that defined it,
    # not the subclass that captured it.
    class OwnT2_Parent
      def hello; "p"; end
    end
    class OwnT2_Child < OwnT2_Parent
    end
    c = OwnT2_Child.new
    assert_eq(c.method(:hello).owner == OwnT2_Parent, true)
    assert_eq(c.method(:hello).owner == OwnT2_Child,  false)
  end

  it "returns the defining class even for an aliased method" do
    # Upstream: "returns the same owner when aliased in the
    # same classes" — both the original and the alias report
    # the class that holds them, not the alias-source class.
    class OwnT3
      def foo; "f"; end
      alias_method :bar, :foo
    end
    obj = OwnT3.new
    assert_eq(obj.method(:foo).owner == OwnT3, true)
    assert_eq(obj.method(:bar).owner == OwnT3, true)
  end

  it "returns String for a built-in String method" do
    # Upstream: `"abc".method(:upcase).owner.should == String`.
    assert_eq("abc".method(:upcase).owner == String, true)
  end
end
