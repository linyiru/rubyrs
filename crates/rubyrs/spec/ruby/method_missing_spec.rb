# Adapted from ruby/spec core/basicobject/method_missing_spec.rb
# at 2026-05 (subset). rubyrs's PoC only triggers
# method_missing on Value::Object receivers, so primitive
# fall-through cases (e.g. `1.foo` routing to Integer's
# method_missing) are out of scope here.

describe "BasicObject#method_missing" do
  it "is invoked for unknown method names on user-class instances" do
    class MMBasic
      def method_missing(name)
        name.to_s
      end
    end
    assert_eq(MMBasic.new.poof, "poof")
    assert_eq(MMBasic.new.boo, "boo")
  end

  it "is inherited through the superclass chain" do
    class MMBase
      def method_missing(name)
        "base/#{name}"
      end
    end
    class MMMid < MMBase
      # no method_missing of our own — should inherit from MMBase
    end
    assert_eq(MMMid.new.unknown_thing, "base/unknown_thing")
  end

  it "receives the missed name as the first arg + splat captures the rest" do
    # The combination that makes method_missing actually
    # useful as a DSL proxy: bare splat in the param list +
    # method_missing fallback.
    class MMProxy
      def method_missing(name, *args)
        "#{name}(#{args.length})"
      end
    end
    p = MMProxy.new
    assert_eq(p.zero, "zero(0)")
    assert_eq(p.one(1), "one(1)")
    assert_eq(p.three(1, 2, 3), "three(3)")
  end

  it "raises NoMethodError when no method_missing is defined" do
    class MMEmpty
    end
    assert_raises("NoMethodError") do
      MMEmpty.new.does_not_exist
    end
  end
end
