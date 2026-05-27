# Adapted from ruby/spec core/hash/size_spec.rb (delegates via
# `it_behaves_like :hash_size, :size` to `shared/length.rb`) at
# upstream commit 448cb340 (2026-05). Hand-translated — the
# shared body's `@method` indirection is inlined as direct
# `.size` calls.
#
# The shared body has two `it` blocks: "returns 0 ..." and
# "returns 0 ... with default values too". Both are covered
# here. Length (`#length`) is the alias; see hash_length_spec.rb.

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
