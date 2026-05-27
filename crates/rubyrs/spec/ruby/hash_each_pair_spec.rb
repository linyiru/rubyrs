# Adapted from ruby/spec core/hash/each_pair_spec.rb +
# shared/each.rb at upstream commit 448cb340 (2026-05).
# Hand-translated — the shared bodies upstream cover "yields
# each pair (key, value)", "returns self", and several
# fixture/Enumerator/iteration-order variants. The runnable
# subset is inlined here. `each` is an alias of `each_pair`
# for Hash in CRuby; rubyrs honours this, so the same `it`
# bodies could run via either method — covered in
# tests/diff/hash_each_key_value.rb (diff_cruby oracle).

describe "Hash#each_pair" do
  it "yields each pair (key, value) to the block" do
    h = { a: 1, b: 2, c: 3 }
    seen = []
    h.each_pair { |k, v| seen << [k, v] }
    assert_eq(seen, [[:a, 1], [:b, 2], [:c, 3]])
  end

  it "returns the receiver" do
    h = { a: 1, b: 2 }
    assert(h.each_pair { |_, _| }.equal?(h))
  end

  it "yields a 2-element array when block takes one arg" do
    h = { a: 1, b: 2 }
    seen = []
    h.each_pair { |pair| seen << pair }
    assert_eq(seen, [[:a, 1], [:b, 2]])
  end

  # skipped (method-not-implemented): it_behaves_like :hash_iteration_no_block, :each_pair
  #   No-block form returns Enumerator (not in subset).
  # skipped (method-not-implemented): it_behaves_like :enumeratorized_with_origin_size
end
