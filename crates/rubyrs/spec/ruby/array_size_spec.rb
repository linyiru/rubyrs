# Adapted from ruby/spec core/array/size_spec.rb + shared/length.rb
# at upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array method FORMS (multi-arg `Array#push`,
# block-form `min { ... }`, count-form `first(n)`), or
# `mock`/`should_receive`; each drop leaves a
# `# skipped (<category>): ...` trace inline. Regenerate by
# re-running the extractor + polish pipeline documented in
# crates/rubyrs-spec-extract/README.md.

describe "Array#size" do
  it "returns the number of elements" do
    assert_eq([].send(:size), 0)
    assert_eq([1, 2, 3].send(:size), 3)
  end

  # skipped (fixture): it "properly handles recursive arrays" do
end
