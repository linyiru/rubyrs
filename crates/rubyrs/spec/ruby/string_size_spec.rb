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

  # Skipped — upstream shared/length.rb:13–18
  #   "returns the length of a string in different encodings"
  # Needs `Encoding::UTF_32BE` / `Encoding::SHIFT_JIS` constants
  # and `String#encode(enc)`. rubyrs is byte-flat UTF-8 only.

  # Skipped — upstream shared/length.rb:20–25
  #   "returns the length of the new self after encoding is changed"
  # Needs `+''` mutable-string literal and `String#force_encoding`.

  # Skipped — upstream shared/length.rb:27–38
  #   "returns the correct length after force_encoding(BINARY)"
  # Needs `String#encoding` returning an Encoding object and
  # `force_encoding(Encoding::ASCII_8BIT)`.

  # Skipped — upstream shared/length.rb:40–44
  #   "adds 1 for every invalid byte in UTF-8"
  # rubyrs's `String#size` is `chars.count` on the UTF-8
  # decoded view; invalid-byte handling diverges from CRuby's
  # "+1 per invalid byte" semantics.

  # Skipped — upstream shared/length.rb:46–49
  #   "adds 1 (and not 2) for an incomplete surrogate in UTF-16"
  # Needs `force_encoding("UTF-16LE"/"UTF-16BE")`.

  # Skipped — upstream shared/length.rb:51–54
  #   "adds 1 for a broken sequence in UTF-32"
  # Needs `force_encoding("UTF-32LE"/"UTF-32BE")`.
end
