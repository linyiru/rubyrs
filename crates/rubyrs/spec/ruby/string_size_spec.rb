# Adapted from ruby/spec core/string/size_spec.rb at upstream
# master (2026-05). The upstream file is `it_behaves_like
# :string_length, :size` against the same `shared/length.rb` —
# extractor v0.4 inlines the shared body with `@method` → `:size`,
# producing identical assertions to string_length_spec.rb but
# against `String#size`. Both consumers of the shared example
# ship as the first v0.4 cross-file dogfood.
#
# Same skipped blocks as string_length_spec.rb — see that
# file's header for the per-block reason (all six relate to
# Encoding features rubyrs doesn't model).

# Same `.send(@method)` → `.size` hand-fix as
# string_length_spec.rb (rubyrs doesn't implement `Object#send`
# yet — see that file's header for the full note).

describe "String#size" do
  it "returns the length of self" do
    assert_eq("".size, 0)
    assert_eq("\x00".size, 1)
    assert_eq("one".size, 3)
    assert_eq("two".size, 3)
    assert_eq("three".size, 5)
    assert_eq("four".size, 4)
  end
end
