# Adapted from ruby/spec core/hash/slice_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — the first three
# blocks (basic slice, missing keys, no-arg) are inlined.
# Blocks depending on Hash-subclass return-type / frozen-bit
# are dropped.

describe "Hash#slice" do
  it "returns a hash with only the requested keys" do
    h = { a: 1, b: 2, c: 3 }
    assert_eq(h.slice(:a, :c), { a: 1, c: 3 })
    # Non-destructive — receiver unchanged.
    assert_eq(h, { a: 1, b: 2, c: 3 })
  end

  it "ignores keys not present in the hash" do
    h = { a: 1, b: 2 }
    assert_eq(h.slice(:missing), {})
    assert_eq(h.slice(:a, :missing), { a: 1 })
  end

  it "returns an empty hash when called with no arguments" do
    h = { a: 1, b: 2 }
    s = h.slice
    assert_eq(s, {})
    assert(!s.equal?(h))
  end

  # skipped (fixture): it "returns a Hash instance, even on subclasses" do
end
