# Adapted from ruby/spec core/string/chars_spec.rb +
# shared/chars.rb at upstream commit 448cb340 (2026-05).
# Hand-translated — the upstream file delegates to
# `it_behaves_like :string_chars, :chars` against shared/chars.rb,
# which in turn uses `send(@method) { ... }` block form, the
# StringSpecs::MyString fixture, and encoding (`force_encoding`,
# Encoding::US_ASCII) — none of which are in the micro-runner
# surface. The own block from the main file plus a hand-added
# empty-string case are kept.

describe "String#chars" do
  it "returns an array when no block given" do
    assert_eq("hello".chars, ['h', 'e', 'l', 'l', 'o'])
  end

  it "returns an empty array for the empty string" do
    assert_eq("".chars, [])
  end

  # skipped (fixture): it "passes each char in self to the given block" do
  #   Block form `chars { |c| ... }` not implemented.
  # skipped (fixture): it "returns Strings in the same encoding as self" do
  #   Encoding::US_ASCII / `encode` not in subset.
  # skipped (fixture): it "is unicode aware" do
  #   Body uses `\303\207` octal escapes + `.to_a` chaining on
  #   the block-form return — micro-runner surface is too narrow.
end
