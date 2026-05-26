# Adapted from ruby/spec core/array/include_spec.rb + shared/index.rb
# at upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array method FORMS (e.g. multi-arg `Array#push`,
# count-form `first(n)` / `last(n)` / `pop(n)` / `shift(n)`,
# block-form `min { ... }` / `max { ... }` / `sort { ... }`),
# or `mock`/`should_receive`; each drop leaves a
# `# skipped (<category>): ...` trace inline. See
# crates/rubyrs-spec-extract/scripts/polish.py DROP_PATTERNS
# for the full set. Regenerate by re-running the extractor
# + polish pipeline documented in
# crates/rubyrs-spec-extract/README.md.

describe "Array#include?" do
  it "returns true if object is present, false otherwise" do
    assert_eq([1, 2, "a", "b"].include?("c"), false)
    assert_eq([1, 2, "a", "b"].include?("a"), true)
  end

  # skipped (mock): it "determines presence by using element == obj" do

  # skipped (mock): it "calls == on elements from left to right until success" do
end
