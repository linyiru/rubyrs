# Adapted from ruby/spec core/string/start_with_spec.rb +
# shared/string/start_with.rb at upstream commit 448cb340
# (2026-05). Hand-translated — the upstream file delegates to
# `it_behaves_like :start_with, :to_s` against shared/start_with.
# Runnable single-arg cases are inlined; multi-arg, mock, regexp,
# and TypeError-on-non-String blocks are skipped (rubyrs's
# `String#start_with?` accepts exactly one String argument).

describe "String#start_with?" do
  it "returns true only if beginning match" do
    assert_eq("hello".start_with?('h'), true)
    assert_eq("hello".start_with?('hel'), true)
    assert_eq("hello".start_with?('el'), false)
  end

  it "returns true if the search string is empty" do
    assert_eq("hello".start_with?(""), true)
    assert_eq("".start_with?(""), true)
  end

  it "works for multibyte strings" do
    assert_eq("céréale".start_with?("cér"), true)
  end

  # skipped (method-not-implemented): it "returns true only if any beginning match" do
  #   Multi-arg form `start_with?('x', 'y', 'he', 'z')` not in subset.
  # skipped (method-not-implemented): it "uses only the needed arguments" do
  # skipped (method-not-implemented): it "ignores arguments not convertible to string" do
  #   Multi-arg + TypeError-on-non-String form not in subset.
  # skipped (mock): it "converts its argument using :to_str" do
  # skipped (method-not-implemented): it "supports regexps" do
  # skipped (fixture): it "matches part of a character with the same part" do
  #   Uses raw `\xA9` byte literals tied to encoding semantics.
  # skipped (fixture): it "checks we are matching only part of a character" do
end
