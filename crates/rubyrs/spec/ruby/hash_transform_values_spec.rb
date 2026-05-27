# Adapted from ruby/spec core/hash/transform_values_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — first
# two blocks of the main describe are inlined. The same
# identity-sharing / Enumerator / Hash-subclass blocks dropped
# as in hash_transform_keys_spec.rb.

describe "Hash#transform_values" do
  it "returns new hash" do
    h = { a: 1, b: 2, c: 3 }
    ret = h.transform_values(&:succ)
    assert(!ret.equal?(h))
    assert_eq(ret.is_a?(Hash), true)
  end

  it "sets the result as transformed values with the given block" do
    h = { a: 1, b: 2, c: 3 }
    assert_eq(h.transform_values(&:succ), { a: 2, b: 3, c: 4 })
  end

  # skipped (method-not-implemented): it "makes both hashes to share keys" do
  #   Identity-sharing assertion on `Hash#keys[0].equal?(key)`.
  # skipped (method-not-implemented): when no block given — returns Enumerator
  # skipped (fixture): it "returns a Hash instance, even on subclasses" do
  # skipped (method-not-implemented): describe "Hash#transform_values!" do ... end
end
