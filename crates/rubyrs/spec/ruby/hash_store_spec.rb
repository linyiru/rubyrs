# Adapted from ruby/spec core/hash/store_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — both the
# main "stores key/value" block and the "returns value" block
# are inlined. CRuby's frozen-hash FrozenError sibling is
# dropped (rubyrs doesn't model the frozen bit at Value level).

describe "Hash#store" do
  it "associates the key with the value and return the value" do
    h = { a: 1 }
    assert_eq(h.store(:b, 2), 2)
    assert_eq(h, { a: 1, b: 2 })
  end

  it "overwrites the value for an existing key" do
    h = { a: 1 }
    h.store(:a, 2)
    assert_eq(h, { a: 2 })
  end

  # skipped (method-not-implemented): it "raises a FrozenError if called on a frozen instance" do
  #   rubyrs doesn't model the frozen bit at Value level —
  #   `Object#freeze` is a no-op, `frozen?` returns false, so
  #   FrozenError can't be raised here.
end
