# Adapted from ruby/spec core/method/to_proc_spec.rb at 2026-05
# (subset). Covers both the user-visible `&meth` coercion path
# AND the now-explicit `Method#to_proc` call (master 1e08224).
#
# Skipped:
#   - `meth.to_proc.arity == meth.arity` — depends on
#     `Method#arity` (separate spec; tracked in the upstream
#     `arity_spec.rb` which uses the SpecEvaluate heredoc form
#     and isn't yet vendored).
#   - `define_method :foo, method(:to_s).to_proc` — define_method
#     accepting a Proc as the second positional arg is a
#     separate dispatch path not yet exercised here.
#   - `instance_exec(4, &5.method(:+))` — depends on
#     `instance_exec` accepting a Proc bound to a different
#     receiver; covered when instance_exec routing extends.

describe "Method#to_proc" do
  it "returns a Proc object" do
    class ToProcT0
      def double(x); x * 2; end
    end
    t = ToProcT0.new
    # Master 1e08224 exposed explicit `.to_proc`; the surface
    # is the same Proc the `&` coercion produces.
    p = t.method(:double).to_proc
    assert_eq(p.class.to_s, "Proc")
  end

  it "returns a Proc that calls the original method" do
    class ToProcT0b
      def triple(x); x * 3; end
    end
    p = ToProcT0b.new.method(:triple).to_proc
    assert_eq(p.call(7), 21)
  end

  it "preserves the receiver across the to_proc bridge" do
    class ToProcT0c
      def initialize(base); @base = base; end
      def add(x); x + @base; end
    end
    p = ToProcT0c.new(100).method(:add).to_proc
    assert_eq(p.call(5), 105)
  end
end

describe "Method#to_proc (via & forwarding)" do
  it "lets a Method be passed as a block via &" do
    class ToProcTarget1
      def double(x); x * 2; end
    end
    t = ToProcTarget1.new
    assert_eq([1, 2, 3].map(&t.method(:double)), [2, 4, 6])
  end

  it "preserves the bound receiver across forwarding" do
    class ToProcTarget2
      def initialize(base); @base = base; end
      def add(x); x + @base; end
    end
    t = ToProcTarget2.new(100)
    assert_eq([1, 2, 3].map(&t.method(:add)), [101, 102, 103])
  end

  it "carries the right method body across the & boundary" do
    # A regression that swapped Methods in the BoundMethod-to-block
    # path would surface as wrong outputs here.
    class ToProcTarget3
      def double(x); x * 2; end
      def triple(x); x * 3; end
    end
    t = ToProcTarget3.new
    assert_eq([1, 2, 3].map(&t.method(:double)), [2, 4, 6])
    assert_eq([1, 2, 3].map(&t.method(:triple)), [3, 6, 9])
  end

  it "works for a single-arg method body" do
    class ToProcTarget4
      def negate(x); -x; end
    end
    t = ToProcTarget4.new
    assert_eq([1, -2, 3].map(&t.method(:negate)), [-1, 2, -3])
  end
end
