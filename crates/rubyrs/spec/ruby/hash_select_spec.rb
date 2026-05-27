# Adapted from ruby/spec core/hash/select_spec.rb +
# shared/select.rb at upstream commit 448cb340 (2026-05).
# Hand-translated — upstream delegates to `shared/select.rb`'s
# `:hash_select` body, which we partially inline. The `select!`
# sibling describe and the order-vs-reject parity block are
# dropped (require `Hash#select!` and `dup` interplay we don't
# need to pin here).

describe "Hash#select" do
  it "yields two arguments: key and value" do
    all_args = []
    { 1 => 2, 3 => 4 }.select { |*args| all_args << args }
    assert_eq(all_args.sort, [[1, 2], [3, 4]])
  end

  it "returns a Hash of entries for which block is true" do
    a_pairs = { 'a' => 9, 'c' => 4, 'b' => 5, 'd' => 2 }.select { |k, v| v % 2 == 0 }
    assert_eq(a_pairs.is_a?(Hash), true)
    assert_eq(a_pairs.sort, [['c', 4], ['d', 2]])
  end

  # skipped (method-not-implemented): it "processes entries with the same order as reject" do
  #   Order-against-reject parity check — not load-bearing here.
  # skipped (method-not-implemented): describe "Hash#select!" do ... end
  #   `Hash#select!` not in subset.
end
