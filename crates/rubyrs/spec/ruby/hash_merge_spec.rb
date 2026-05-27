# Adapted from ruby/spec core/hash/merge_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — first block of
# the main describe is inlined (covers basic single-arg merge,
# empty-hash identities, dup-key override). The block-form
# merge (`merge(h) { |k,x,y| ... }`) is skipped — rubyrs's
# `Hash#merge` accepts a single Hash arg only; the block form
# raises NoMethodError. mock-based `to_hash` blocks + MyHash
# subclass blocks dropped.

describe "Hash#merge" do
  it "returns a new hash by combining self with the contents of other" do
    h = { 1 => :a, 2 => :b, 3 => :c }.merge(a: 1, c: 2)
    assert_eq(h, { c: 2, 1 => :a, 2 => :b, a: 1, 3 => :c })

    # Pin non-mutating behavior: `merge` must return a fresh
    # Hash, not the receiver or the argument — value equality
    # alone would still pass if either were returned in place.
    hash = { a: 1, b: 2 }
    empty_merge = {}.merge(hash)
    assert_eq(empty_merge, hash)
    assert(!empty_merge.equal?(hash))
    self_empty = hash.merge({})
    assert_eq(self_empty, hash)
    assert(!self_empty.equal?(hash))

    h = { 1 => :a, 2 => :b, 3 => :c }.merge(1 => :b)
    assert_eq(h, { 1 => :b, 2 => :b, 3 => :c })

    h = { 1 => :a, 2 => :b }.merge(1 => :b, 3 => :c)
    assert_eq(h, { 1 => :b, 2 => :b, 3 => :c })
  end

  # skipped (method-not-implemented): it "sets any duplicate key to the value of block if passed a block" do
  #   Block-form merge (`merge(h) { |k,x,y| ... }`) not in subset.
  # skipped (mock): it "tries to convert the passed argument to a hash using #to_hash" do
  # skipped (fixture): it "does not call to_hash on hash subclasses" do
  # skipped (fixture): it "returns subclass instance for subclasses" do
  # skipped (method-not-implemented): it "preserves the order of the original" do
  # skipped (method-not-implemented): describe "Hash#merge!" do ... end
end
