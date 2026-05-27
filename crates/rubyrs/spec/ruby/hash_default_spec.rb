# Adapted from ruby/spec core/hash/default_spec.rb at upstream
# commit 448cb340 (2026-05). Hand-translated — the no-arg form
# is inlined: returns scalar default value (or nil) without
# invoking the default_proc.

describe "Hash#default" do
  it "returns nil for a hash without a default" do
    assert_eq({}.default, nil)
    assert_eq({ a: 1 }.default, nil)
  end

  it "returns the scalar default set via Hash.new(value)" do
    assert_eq(Hash.new(99).default, 99)
    assert_eq(Hash.new("missing").default, "missing")
  end

  it "returns nil for a hash with a default_proc (not the proc)" do
    h = Hash.new { |hh, k| k.to_s }
    assert_eq(h.default, nil)
  end

  # skipped (method-not-implemented): it "invokes default_proc when called with a key" do
  #   `h.default(key)` invokes the default_proc with (self, key)
  #   and returns the result. Requires the step_block scaffold
  #   the `[]` lookup-miss arm uses; out of subset.
end
