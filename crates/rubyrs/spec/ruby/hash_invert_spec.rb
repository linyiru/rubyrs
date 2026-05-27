# Adapted from ruby/spec core/hash/invert_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — the first three
# blocks fit the micro-runner surface (`#invert` with basic
# value-uniqueness, collision-by-later-key, and `eql?` semantics
# on Float vs Int keys). The rest depend on `HashSpecs::MyHash`
# subclass fixture (1 block), `Hash#default` (2 blocks),
# `default_proc` (1 block), or `compare_by_identity` (1 block) —
# none are in the micro-runner's surface.

describe "Hash#invert" do
  it "returns a new hash where keys are values and vice versa" do
    assert_eq({ 1 => 'a', 2 => 'b', 3 => 'c' }.invert,
              { 'a' => 1, 'b' => 2, 'c' => 3 })
  end

  it "handles collisions by overriding with the key coming later in keys()" do
    h = { a: 1, b: 1 }
    override_key = h.keys.last
    assert_eq(h.invert[1], override_key)
  end

  # skipped (divergent): it "compares new keys with eql? semantics" do
  #   CRuby's Hash uses `eql?` so `{ a: 1.0, b: 1 }` keeps both
  #   entries (1.0 != 1 under eql?). rubyrs's Hash collapses
  #   numerically-equal keys, so `{ a: 1.0, b: 1 }` becomes
  #   `{ 1.0 => :b }` (the later write wins) and `invert[1] ==
  #   invert[1.0] == :b`. Behavior divergence, not unimplemented;
  #   would need Float/Int eql? distinction in Hash key lookup.

  # skipped (fixture): it "does not return subclass instances for subclasses" do
  #   Uses `HashSpecs::MyHash`.
  # skipped (method-not-implemented): it "does not retain the default value" do
  #   `Hash#default` not in subset.
  # skipped (method-not-implemented): it "does not retain the default_proc" do
  # skipped (method-not-implemented): it "does not retain compare_by_identity flag" do
end
