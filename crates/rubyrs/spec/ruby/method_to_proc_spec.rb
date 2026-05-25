# Adapted from ruby/spec core/method/to_proc_spec.rb at 2026-05
# (subset).
#
# The upstream file tests `Method#to_proc` directly. rubyrs's
# `&method`-forwarding implements `to_proc` implicitly inside
# the `&` coercion path but does not yet expose `Method#to_proc`
# as a user-callable method (calling it raises NoMethodError).
# This spec exercises the surface that IS user-reachable —
# the implicit `&` coercion — so any regression in
# BoundMethod-to-block routing fails here.
#
# Skipped (out of master):
#   - `meth.to_proc.kind_of?(Proc)` — direct `.to_proc` raises
#     NoMethodError today. The implicit `&` form below covers
#     the user-visible behaviour.
#   - `meth.to_proc.arity == meth.arity` — depends on `Method#arity`
#     + a separately-callable to_proc.
#   - `define_method :foo, method(:to_s).to_proc` — same
#     to_proc-as-method gap.
#   - `instance_exec(4, &5.method(:+))` — depends on Integer#+
#     bound-method capture going through to_proc; covered when
#     to_proc is exposed.

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
