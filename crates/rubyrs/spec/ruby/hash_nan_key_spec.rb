# Hash key operations with Float::NAN — pins the NaN
# identity shortcut in `ruby_eql` for Hash lookup contexts
# (Hash#[], Hash#[]=, Hash#include?). See heap.rs:1004
# `ruby_eql` doc for the broader rationale.

describe "Hash with Float::NAN keys" do
  it "looks up a NaN key inserted via the same nan local" do
    # Common idiom: store under nan, retrieve under nan.
    # Without the NaN identity shortcut in ruby_eql, the
    # lookup would fail because `NaN == NaN` is false in
    # IEEE 754 — Hash collision check would never match.
    nan = 0.0 / 0.0
    h = {nan => 1}
    assert_eq(h[nan], 1)
    assert_eq(h.include?(nan), true)
  end

  it "overwrites a NaN-keyed entry rather than inserting a duplicate" do
    nan = 0.0 / 0.0
    h = {nan => 1}
    h[nan] = 99
    assert_eq(h.size, 1)
    assert_eq(h[nan], 99)
  end

  it "treats bit-identical NaN values as the same key (rubyrs value-Float semantics)" do
    # CRuby treats distinct Float OBJECTS as distinct
    # keys even when their NaN bits match — so
    # `{(0.0/0.0) => 1}[0.0/0.0]` returns nil. rubyrs has
    # value-based Floats with no identity, so two NaN
    # values with matching bits ARE the same value and
    # lookup succeeds. Documented divergence.
    h = {(0.0 / 0.0) => 1}
    assert_eq(h[0.0 / 0.0], 1)  # CRuby returns nil here
  end

  it "still misses non-NaN unrelated keys" do
    # Sanity: NaN shortcut doesn't accidentally short-
    # circuit normal Float comparisons.
    h = {1.5 => :a}
    assert_eq(h[2.5], nil)
    assert_eq(h[1.5], :a)
  end
end
