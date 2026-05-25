# Adapted from ruby/spec core/method/compose_spec.rb at 2026-05
# (subset). Skipped:
#   - `it_behaves_like :proc_compose, ...` shared examples
#   - `is_a?(Proc)` / `lambda?` predicate checks on the result
#     (rubyrs returns a Proc; `lambda?` not modelled)
#   - "accepts any callable object" — depends on Object.new +
#     def obj.call dispatch through call protocol, exercised
#     elsewhere
#   - MethodSpecs::Composition fixtures

describe "Method#<<" do
  it "returns a Proc that runs the passed Proc first, then self" do
    # m << p  ==  ->(x) { m.call(p.call(x)) }
    class ComposeLT1
      def double(x); x * 2; end
    end
    m_double = ComposeLT1.new.method(:double)
    add5 = proc { |x| x + 5 }
    composed = m_double << add5
    # add5(3) = 8, double(8) = 16
    assert_eq(composed.call(3), 16)
  end

  it "calls the passed Proc with multiple arguments" do
    class ComposeLT2
      def inc(x); x + 1; end
    end
    m_inc = ComposeLT2.new.method(:inc)
    mul = proc { |a, b| a * b }
    composed = m_inc << mul
    # mul(2, 3) = 6, inc(6) = 7
    assert_eq(composed.call(2, 3), 7)
  end

  it "composes Method with Method (both sides are callables)" do
    class ComposeLT3
      def double(x); x * 2; end
      def inc(x); x + 1; end
    end
    o = ComposeLT3.new
    m_double = o.method(:double)
    m_inc    = o.method(:inc)
    # double << inc  ==  double(inc(x))
    composed = m_double << m_inc
    assert_eq(composed.call(4), 10)  # inc(4)=5, double(5)=10
  end
end

describe "Method#>>" do
  it "returns a Proc that runs self first, then the passed Proc" do
    # m >> p  ==  ->(x) { p.call(m.call(x)) }
    class ComposeGT1
      def double(x); x * 2; end
    end
    m_double = ComposeGT1.new.method(:double)
    add5 = proc { |x| x + 5 }
    composed = m_double >> add5
    # double(3) = 6, add5(6) = 11
    assert_eq(composed.call(3), 11)
  end

  it "may accept multiple arguments to the leading Method" do
    class ComposeGT2
      def add(a, b); a + b; end
    end
    m_add = ComposeGT2.new.method(:add)
    double = proc { |x| x * 2 }
    composed = m_add >> double
    # add(2, 3) = 5, double(5) = 10
    assert_eq(composed.call(2, 3), 10)
  end

  it "composes Method with Method" do
    class ComposeGT3
      def double(x); x * 2; end
      def inc(x); x + 1; end
    end
    o = ComposeGT3.new
    composed = o.method(:double) >> o.method(:inc)
    assert_eq(composed.call(4), 9)  # double(4)=8, inc(8)=9
  end
end
