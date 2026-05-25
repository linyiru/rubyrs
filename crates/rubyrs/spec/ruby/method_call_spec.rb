# Adapted from ruby/spec core/method/call_spec.rb (+ shared/call.rb)
# at 2026-05 (subset). Upstream's whole file delegates to
# shared/call.rb via `it_behaves_like :method_call, :call` which
# also covers `Method#()` (the shorthand call form). We inline
# both surfaces here. Skipped:
#   - respond_to_missing? + method_missing-generated Method
#     dispatch (separate spec area — method_missing has its
#     own spec/ruby/method_missing_spec.rb)
#   - attr= setter routed through Method#call (covered via
#     other test infrastructure)
#   - MethodSpecs::Methods fixture file — defines named classes
#     inline per `it` block instead

describe "Method#call" do
  it "invokes the method with the specified arguments" do
    class CallTarget1
      def add(a, b)
        a + b
      end
    end
    m = CallTarget1.new.method(:add)
    assert_eq(m.call(2, 3), 5)
    assert_eq(m.call(10, 20), 30)
  end

  it "returns the method's return value" do
    class CallTarget2
      def name; "ruby"; end
    end
    m = CallTarget2.new.method(:name)
    assert_eq(m.call, "ruby")
  end

  it "raises ArgumentError when given too many arguments" do
    class CallTarget3
      def two(a, b); a + b; end
    end
    m = CallTarget3.new.method(:two)
    assert_raises("ArgumentError") do
      m.call(1, 2, 3)
    end
  end

  it "raises ArgumentError when given too few arguments" do
    class CallTarget4
      def two(a, b); a + b; end
    end
    m = CallTarget4.new.method(:two)
    assert_raises("ArgumentError") do
      m.call(1)
    end
  end

  it "preserves self of the receiver the Method was bound to" do
    # The Method captures the receiver; calling it from elsewhere
    # still routes to the original object's state.
    class CallTarget5
      def initialize(label)
        @label = label
      end
      def echo
        @label
      end
    end
    a = CallTarget5.new("A")
    b = CallTarget5.new("B")
    m_a = a.method(:echo)
    assert_eq(m_a.call, "A")
    # Verify another instance's Method captures its own receiver.
    m_b = b.method(:echo)
    assert_eq(m_b.call, "B")
  end
end

describe "Method#()" do
  # Upstream tests `.()` via the shared :method_call helper too;
  # rubyrs parses `m.(args)` as sugar for `m.call(args)`.
  it "is shorthand for .call" do
    class CallTarget6
      def triple(x); x * 3; end
    end
    m = CallTarget6.new.method(:triple)
    assert_eq(m.(4), 12)
    assert_eq(m.(0), 0)
  end
end
