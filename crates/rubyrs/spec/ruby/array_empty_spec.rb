# Adapted from ruby/spec core/array/empty_spec.rb at
# upstream commit 448cb340 (2026-05). Produced by
# `rubyrs-spec-extract` v0.4 + `scripts/polish.py`.
#
# polish.py dropped `it` blocks containing fixture refs,
# unimplemented Array method FORMS (multi-arg `Array#push`,
# block-form `min { ... }`, count-form `first(n)`), or
# `mock`/`should_receive`; each drop leaves a
# `# skipped (<category>): ...` trace inline. Regenerate by
# re-running the extractor + polish pipeline documented in
# crates/rubyrs-spec-extract/README.md.

describe "Array#empty?" do
  it "returns true if the array has no elements" do
    assert([].empty?)
    assert(![1].empty?)
    assert(![1, 2].empty?)
  end
end
