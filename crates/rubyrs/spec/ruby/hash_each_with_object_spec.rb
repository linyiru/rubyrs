# Adapted from ruby/spec core/enumerable/each_with_object_spec.rb
# at upstream commit 448cb340 (2026-05). Hand-translated — the
# core "threads memo, returns memo" + "block return ignored"
# semantics on Hash receivers. Enumerator no-block form dropped.

describe "Hash#each_with_object" do
  it "threads memo across iterations and returns it" do
    h = { a: 1, b: 2, c: 3 }
    result = h.each_with_object([]) { |(k, v), acc| acc << [k, v] }
    assert_eq(result, [[:a, 1], [:b, 2], [:c, 3]])
  end

  it "ignores the block's return value (memo is the result)" do
    h = { a: 1, b: 2 }
    result = h.each_with_object([]) { |(k, v), acc| acc << v; :ignored }
    assert_eq(result, [1, 2])
  end

  it "works with a Hash memo for invert-style transforms" do
    h = { a: 1, b: 2 }
    result = h.each_with_object({}) { |(k, v), m| m[v] = k }
    assert_eq(result, { 1 => :a, 2 => :b })
  end

  # skipped (method-not-implemented): no-block Enumerator form.
end
