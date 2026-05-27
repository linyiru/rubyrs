# Adapted from ruby/spec core/hash/transform_keys_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — first
# three blocks of the main describe are inlined. Blocks
# depending on Hash subclass / Enumerator (no-block form) /
# the keyword-arg mapping shape are dropped. Upstream uses
# Symbol keys with `&:succ`; rubyrs's subset has `String#succ`
# but not `Symbol#succ`, so the keys are switched to strings.

describe "Hash#transform_keys" do
  it "returns new hash" do
    h = { "a" => 1, "b" => 2, "c" => 3 }
    ret = h.transform_keys(&:succ)
    assert(!ret.equal?(h))
    assert_eq(ret.is_a?(Hash), true)
  end

  it "sets the result as transformed keys with the given block" do
    h = { "a" => 1, "b" => 2, "c" => 3 }
    assert_eq(h.transform_keys(&:succ), { "b" => 1, "c" => 2, "d" => 3 })
  end

  it "keeps last pair if new keys conflict" do
    h = { a: 1, b: 2, c: 3 }
    assert_eq(h.transform_keys { |_| :a }, { a: 3 })
  end

  # skipped (method-not-implemented): it "makes both hashes to share values" do
  #   Uses `equal?` for value identity assertion — would need
  #   the shared structural representation to actually share.
  # skipped (method-not-implemented): when no block given — returns Enumerator
  # skipped (fixture): it "returns a Hash instance, even on subclasses" do
  # skipped (method-not-implemented): describe "Hash#transform_keys!" do ... end
  # skipped (method-not-implemented): kwarg-mapping form (transform_keys(a: :z, b: :y))
end
