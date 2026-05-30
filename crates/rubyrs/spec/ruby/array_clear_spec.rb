# Adapted from ruby/spec core/array/clear_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — frozen-array
# fixture (`ArraySpecs.frozen_array`) and tainted-string
# variants are dropped (rubyrs doesn't track taint, and the
# frozen-array fixture isn't vendored).

describe "Array#clear" do
  it "removes all elements" do
    a = [1, 2, 3, 4]
    a.clear
    assert_eq(a, [])
  end

  it "returns self" do
    a = [1]
    assert(a.clear.equal?(a))
  end

  it "is a no-op on an already-empty Array" do
    a = []
    assert(a.clear.equal?(a))
    assert_eq(a, [])
  end

  it "leaves the receiver mutable (push works after clear)" do
    a = [1, 2, 3]
    a.clear
    a << 99
    assert_eq(a, [99])
  end

  it "raises ArgumentError when called with arguments" do
    assert_raises("ArgumentError") { [1].clear(0) }
  end

  it "silently discards a block (CRuby parity)" do
    a = [1, 2, 3]
    r = a.clear { 99 }
    assert(r.equal?(a))
    assert_eq(a, [])
  end

  # Skipped (fixture-not-vendored): the upstream spec's
  # frozen-array case (`ArraySpecs.frozen_array.clear`) raises
  # FrozenError. rubyrs supports `freeze` on Arrays but the
  # ruby/spec fixture module isn't pulled in for the
  # micro-runner.
end
