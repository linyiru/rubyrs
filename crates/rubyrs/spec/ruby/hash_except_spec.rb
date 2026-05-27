# Adapted from ruby/spec core/hash/except_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — three blocks
# (basic except, no-arg fresh-copy, ignore-missing-keys) are
# inlined. Blocks depending on Hash-subclass / frozen-bit /
# `to_hash` mocks are dropped.

describe "Hash#except" do
  it "returns a hash without the given keys" do
    h = { a: 1, b: 2, c: 3, d: 4 }
    assert_eq(h.except(:a, :c), { b: 2, d: 4 })
    # Non-destructive — receiver unchanged.
    assert_eq(h, { a: 1, b: 2, c: 3, d: 4 })
  end

  it "returns a copy when no keys given" do
    h = { a: 1, b: 2 }
    e = h.except
    assert_eq(e, { a: 1, b: 2 })
    assert(!e.equal?(h))
  end

  it "ignores keys not present in the hash" do
    h = { a: 1, b: 2 }
    assert_eq(h.except(:c, :d), { a: 1, b: 2 })
  end

  # skipped (fixture): it "returns a Hash instance, even on subclasses" do
end
