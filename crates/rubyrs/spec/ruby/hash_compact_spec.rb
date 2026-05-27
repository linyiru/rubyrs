# Adapted from ruby/spec core/hash/compact_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — first two
# blocks of the main describe are inlined. The default-value
# / default_proc blocks need `Hash#default` (not in subset);
# the MyHash subclass + compact! sibling describe also dropped.

describe "Hash#compact" do
  it "returns new object that rejects pair has nil value" do
    h = { truthy: true, false: false, nil: nil, nil => true }
    compact = { truthy: true, false: false, nil => true }
    ret = h.compact
    assert(!ret.equal?(h))
    assert_eq(ret, compact)
  end

  it "keeps own pairs" do
    h = { truthy: true, false: false, nil: nil, nil => true }
    initial = h.dup
    h.compact
    assert_eq(h, initial)
  end

  # skipped (method-not-implemented): it "retains the default value" do
  # skipped (method-not-implemented): it "retains the default_proc" do
  #   `Hash#default` / `default_proc` accessors not in subset.
  # skipped (fixture): it "does not return subclass instance for subclasses" do
  # skipped (method-not-implemented): describe "Hash#compact!" do ... end
end
