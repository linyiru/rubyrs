# Adapted from ruby/spec core/unboundmethod/equal_value_spec.rb
# at 2026-05 (subset). Tests `UnboundMethod#==` — sibling to
# the `Method#==` set in method_equal_spec.rb. Master ships
# UnboundMethod#== alongside Method#== (commit 3f6260f).
#
# Skipped:
#   - UnboundMethodSpecs::* fixture classes (inline classes
#     per `it` block instead)
#   - `before :all` setup (each `it` is self-contained)
#   - Module include / mixin equality (depends on Module
#     ancestor chain semantics — covered separately)
#   - Aliased-instance-method equality (same root cause as
#     `Method#==` alias divergence — see docs/SUBSET.md
#     → "Method objects")

describe "Class#instance_method / Method#unbind" do
  it "both return an UnboundMethod" do
    class UnbT1
      def m; 1; end
    end
    assert_eq(UnbT1.instance_method(:m).class.to_s, "UnboundMethod")
    assert_eq(UnbT1.new.method(:m).unbind.class.to_s, "UnboundMethod")
  end
end

describe "UnboundMethod#==" do
  it "returns true when comparing an UnboundMethod to itself" do
    class UnbT2
      def m; 1; end
    end
    um = UnbT2.instance_method(:m)
    assert_eq(um.==(um), true)
  end

  it "returns true for two captures of the same instance_method" do
    class UnbT3
      def m; 1; end
    end
    a = UnbT3.instance_method(:m)
    b = UnbT3.instance_method(:m)
    assert_eq(a.==(b), true)
  end

  it "returns true when comparing instance_method with method(...).unbind" do
    # Upstream: "there is no difference between Method#unbind
    # and Module#instance_method".
    class UnbT4
      def m; 1; end
    end
    from_class    = UnbT4.instance_method(:m)
    from_instance = UnbT4.new.method(:m).unbind
    assert_eq(from_class.==(from_instance), true)
  end

  it "returns false for different method names on the same class" do
    class UnbT5
      def m1; 1; end
      def m2; 2; end
    end
    assert_eq(UnbT5.instance_method(:m1).==(UnbT5.instance_method(:m2)), false)
  end

  it "returns false for methods with identical bodies but different names" do
    class UnbT6
      def m1; 1; end
      def m2; 1; end  # same body, different name
    end
    assert_eq(UnbT6.instance_method(:m1).==(UnbT6.instance_method(:m2)), false)
  end
end
