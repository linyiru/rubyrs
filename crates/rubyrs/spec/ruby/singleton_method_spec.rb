# Adapted from ruby/spec core/basicobject/singleton_method_spec.rb
# + core/kernel/define_singleton_method_spec.rb at 2026-05
# (subset). PoC scope: only user-class instances. Singleton
# methods on primitives (`def 1.foo`, `def "x".bar`) are
# rejected with TypeError — see SUBSET.md's Metaprogramming
# (PoC) caveats.

describe "def obj.name" do
  it "installs a method visible only on that one receiver" do
    class SMHost
    end
    a = SMHost.new
    b = SMHost.new
    def a.label
      "from-a"
    end
    assert_eq(a.label, "from-a")
    # b doesn't see a's singleton method.
    assert_raises("NoMethodError") do
      b.label
    end
  end

  it "leaves obj.class returning the user-declared class, not the eigenclass" do
    # CRuby skips the eigenclass when reporting Object#class —
    # rubyrs matches via `Heap::real_class_of` even when a
    # singleton method has been installed.
    class SMClassReport
    end
    o = SMClassReport.new
    def o.extra
      "noise"
    end
    assert_eq(o.class.to_s, "SMClassReport")
  end

  it "raises TypeError when the receiver isn't a user-class instance" do
    # PoC limitation: only Value::Object receivers get a
    # synthetic eigenclass. Primitives would need per-Value
    # eigenclass plumbing we don't ship yet.
    assert_raises("TypeError") do
      x = 42
      def x.shout
        "loud"
      end
    end
  end

  it "lets super inside the singleton method walk the original class chain" do
    # Singleton class's superclass is the receiver's real
    # class, so super from inside def obj.foo finds Foo#foo.
    class SMParent
      def hello
        "parent"
      end
    end
    o = SMParent.new
    def o.hello
      super + "+singleton"
    end
    assert_eq(o.hello, "parent+singleton")
  end
end

describe "Object#define_singleton_method" do
  it "installs the block as a singleton method on the receiver" do
    class DSMHost
    end
    a = DSMHost.new
    b = DSMHost.new
    a.define_singleton_method(:tag) { "tagged-a" }
    assert_eq(a.tag, "tagged-a")
    assert_raises("NoMethodError") do
      b.tag
    end
  end

  it "closes over the lexical scope — writes propagate across calls" do
    # Same closure semantic that distinguishes define_method
    # from def. The captured slot is shared across all
    # invocations of the same singleton method.
    class DSMCounter
    end
    o = DSMCounter.new
    counter = 0
    o.define_singleton_method(:bump) { counter = counter + 1; counter }
    assert_eq(o.bump, 1)
    assert_eq(o.bump, 2)
    assert_eq(o.bump, 3)
  end

  it "validates arity against the block's declared params" do
    class DSMArity
    end
    o = DSMArity.new
    o.define_singleton_method(:two) { |a, b| a + b }
    assert_raises("ArgumentError") do
      o.two(1)
    end
  end
end
