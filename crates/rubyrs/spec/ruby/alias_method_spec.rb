# Adapted from ruby/spec core/module/alias_method_spec.rb at
# 2026-05 (subset). Each `it` block is self-contained — no
# anonymous Class.new, no mock/mock_function, no `.should ==`.

describe "Module#alias_method" do
  it "adds a new name for an existing method" do
    class AliasBasic
      def hello
        "hi"
      end
      alias_method :greet, :hello
    end
    g = AliasBasic.new
    assert_eq(g.greet, "hi")
    # Original survives — alias doesn't move, it duplicates.
    assert_eq(g.hello, "hi")
  end

  it "shares super-chain semantics with the original" do
    # Aliasing preserves defining_class, so super from the
    # aliased name walks the original's superclass chain
    # (CRuby's "module of definition" rule).
    class AliasParent
      def hello
        "parent"
      end
    end
    class AliasChild < AliasParent
      def hello
        super + "+child"
      end
      alias_method :greet, :hello
    end
    assert_eq(AliasChild.new.greet, "parent+child")
  end

  it "can alias an inherited method" do
    class AliasParent2
      def parent_method
        "from-parent"
      end
    end
    class AliasChild2 < AliasParent2
      alias_method :inherited_alias, :parent_method
    end
    assert_eq(AliasChild2.new.inherited_alias, "from-parent")
  end

  it "raises NameError when the source method doesn't exist" do
    assert_raises("NameError") do
      class AliasBadSource
        alias_method :a, :nonexistent_method
      end
    end
  end

  it "stays stack-balanced when multiple aliases appear in one class body" do
    # Regression for PR #8 review (compiler.rs:486): a stray
    # LoadNil after Op::AliasMethod left one Nil per alias on
    # the operand stack. Locked in here too — three aliases,
    # then a body that consumes a value to confirm balance.
    class AliasMulti
      def a; 1; end
      def b; 2; end
      def c; 3; end
      alias_method :x, :a
      alias_method :y, :b
      alias_method :z, :c
    end
    m = AliasMulti.new
    assert_eq(m.x + m.y + m.z, 6)
  end
end
