# Adapted from ruby/spec core/hash/empty_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — both upstream
# blocks fit the micro-runner surface (predicate matcher
# `.should.empty?` → `assert_eq(..., true)`, plus
# `Hash.new(5)` / `Hash.new { ... }` / `Hash.new { |h,k| ... }`
# default-value constructors which rubyrs implements).

describe "Hash#empty?" do
  it "returns true if the hash has no entries" do
    assert_eq({}.empty?, true)
    assert_eq({ 1 => 1 }.empty?, false)
  end

  it "returns true if the hash has no entries and has a default value" do
    assert_eq(Hash.new(5).empty?, true)
    assert_eq(Hash.new { 5 }.empty?, true)
    assert_eq(Hash.new { |hsh, k| hsh[k] = k }.empty?, true)
  end
end
