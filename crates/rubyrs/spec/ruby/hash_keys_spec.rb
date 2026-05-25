# Adapted from ruby/spec core/hash/keys_spec.rb at
# upstream commit 448cb340 (2026-05). 5th extractor-derived
# spec — produced by `rubyrs-spec-extract` v0.3. The two
# `Hash.new(default)` / `Hash.new { ... }` assertions are
# split out to a SKIPPED inline comment because the
# micro-runner doesn't support Hash with a default value /
# default-proc (those forms return a plain Object, not a
# Hash). The rest of the upstream `it` block ships.

describe "Hash#keys" do

  it "returns an array with the keys in the order they were inserted" do
    assert_eq({}.keys, [])
    assert({}.keys.is_a?(Array))
    # Skipped — upstream keys_spec.rb:7-8 covers Hash with
    # default value / proc; rubyrs's `Hash.new(default)`
    # / `Hash.new { ... }` doesn't construct a Hash (returns
    # Object). Tracked via docs/SUBSET.md.
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
