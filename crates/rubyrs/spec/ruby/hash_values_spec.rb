# Adapted from ruby/spec core/hash/values_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — the upstream
# block uses `should.is_a?(Array)` (predicate matcher) plus
# `sort { |a,b| ... }` block form (both work in rubyrs).

describe "Hash#values" do
  it "returns an array of values" do
    h = { 1 => :a, 'a' => :a, 'the' => 'lang' }
    assert_eq(h.values.is_a?(Array), true)
    assert_eq(h.values.sort { |a, b| a.to_s <=> b.to_s }, [:a, :a, 'lang'])
  end
end
