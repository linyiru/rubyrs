# Adapted from ruby/spec core/array/length_spec.rb + shared/length.rb
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

describe "Array#length" do
  it "returns the number of elements" do
    assert_eq([].send(:length), 0)
    assert_eq([1, 2, 3].send(:length), 3)
  end

  # skipped (fixture): it "properly handles recursive arrays" do
end
