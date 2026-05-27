# Adapted from ruby/spec core/hash/size_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — the upstream
# file delegates to a shared body via it_behaves_like; the
# `@method` indirection there is inlined as direct `.size`
# calls. Two upstream blocks are covered ("returns the count"
# + "is unaffected by the default value"). `Hash#length` is an
# alias of `Hash#size` in CRuby; its own describe is included
# below rather than split into a sibling file.

describe "Hash#size" do
  it "returns the number of entries" do
    assert_eq({ a: 1, b: 2 }.size, 2)
    assert_eq({}.size, 0)
    assert_eq({ a: 1 }.size, 1)
  end

  it "is unaffected by the default value" do
    assert_eq(Hash.new(5).size, 0)
    assert_eq(Hash.new { 99 }.size, 0)
  end
end

describe "Hash#length" do
  it "behaves the same as Hash#size" do
    assert_eq({ a: 1, b: 2 }.length, { a: 1, b: 2 }.size)
    assert_eq({}.length, 0)
  end
end
