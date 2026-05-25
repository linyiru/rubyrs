# Adapted from ruby/spec core/hash/keys_spec.rb at
# upstream commit 448cb340 (2026-05). 5th extractor-derived
# spec — produced by `rubyrs-spec-extract` v0.3. The two
# `Hash.new(default)` / `Hash.new { ... }` assertions are
# split out to a SKIPPED inline comment because rubyrs
# itself doesn't yet construct a Hash from those forms —
# `Hash.new(5)` returns a plain Object, so any Hash method
# on the result (`.keys`, `.[]`) raises NoMethodError
# before the micro-runner sees it. See docs/SUBSET.md →
# "Hash built-in methods" for the runtime-level divergence.
# The rest of the upstream `it` block ships.

describe "Hash#keys" do

  it "returns an array with the keys in the order they were inserted" do
    assert_eq({}.keys, [])
    assert({}.keys.is_a?(Array))
    # Skipped — upstream keys_spec.rb:7-8 covers Hash with
    # default value / proc; rubyrs's `Hash.new(default)` /
    # `Hash.new { ... }` returns an Object, not a Hash. See
    # docs/SUBSET.md → "Hash built-in methods" for the
    # divergence entry.
    #   assert_eq(Hash.new(5).keys, [])
    #   assert_eq(Hash.new { 5 }.keys, [])
    assert_eq({ 1 => 2, 4 => 8, 2 => 4 }.keys, [1, 4, 2])
    assert({ 1 => 2, 2 => 4, 4 => 8 }.keys.is_a?(Array))
    assert_eq({ nil => nil }.keys, [nil])
  end

  it "uses the same order as #values" do
    h = { 1 => "1", 2 => "2", 3 => "3", 4 => "4" }

    h.size.times do |i|
      assert_eq(h[h.keys[i]], h.values[i])
    end
  end
end
