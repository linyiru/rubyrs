# Adapted from ruby/spec core/hash/default_proc_spec.rb at
# upstream commit 448cb340 (2026-05). Hand-translated — the
# reader-side surface is inlined. The setter (`default_proc=`)
# is dropped as method-not-implemented.

describe "Hash#default_proc" do
  it "returns nil for a hash without a default_proc" do
    assert_eq({}.default_proc, nil)
    assert_eq(Hash.new(99).default_proc, nil)
  end

  it "returns the proc passed to Hash.new" do
    h = Hash.new { |hh, k| k.to_s }
    p = h.default_proc
    assert_eq(p.class.to_s, "Proc")
  end

  # skipped (method-not-implemented): describe "Hash#default_proc=" do ... end
  #   Setter is out of subset.
end
