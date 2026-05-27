# Adapted from ruby/spec core/string/end_with_spec.rb +
# shared/string/end_with.rb at upstream commit 448cb340
# (2026-05). Hand-translated — the upstream file delegates to
# `it_behaves_like :end_with, :to_s` against shared/end_with.
# Runnable single-arg cases are inlined; multi-arg, mock,
# regexp blocks are skipped (rubyrs's `String#end_with?`
# accepts exactly one String argument).

describe "String#end_with?" do
  it "returns true only if ends match" do
    assert_eq("hello".end_with?('o'), true)
    assert_eq("hello".end_with?('llo'), true)
  end

  it "returns false if the end does not match" do
    assert_eq("hello".end_with?('ll'), false)
  end

  it "returns true if the search string is empty" do
    assert_eq("hello".end_with?(""), true)
    assert_eq("".end_with?(""), true)
  end

  # skipped (method-not-implemented): it "returns true only if any ending match" do
  #   Multi-arg form not in subset.
  # skipped (mock): it "converts its argument using :to_str" do
  # skipped (method-not-implemented): it "<more multi-arg / regexp blocks>"
end
