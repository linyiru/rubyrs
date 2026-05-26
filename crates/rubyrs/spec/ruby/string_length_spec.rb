# Adapted from ruby/spec core/string/length_spec.rb at
# upstream master (2026-05), plus core/string/shared/length.rb.
# First extractor v0.4 output — produced via:
#   rubyrs-spec-extract upstream/length_spec.rb \
#     --shared upstream/shared/length.rb
# which inlined the `it_behaves_like :string_length, :length`
# call against the shared body and substituted `@method` → `:length`.
#
# Hand-fix on top of the extractor output: the shared body
# calls `"".send(@method)` rather than `"".length` directly,
# so v0.4 emits `"".send(:length)` verbatim. rubyrs doesn't
# implement `Object#send` yet (subset gap surfaced by this
# dogfood), so each `.send(:length)` is rewritten to `.length`
# below. When `Object#send` lands, re-running the extractor
# will produce a smaller diff (just the `:length` symbol
# substitution) and this hand-fix step can be retired.
#
# Six of the upstream shared-body `it` blocks need features
# rubyrs doesn't model — Encoding objects (`Encoding::UTF_32BE`,
# `Encoding::SHIFT_JIS`), `String#encode`, `String#force_encoding`,
# `String#encoding`, and the "invalid byte adds 1" semantics.
# Those are commented out below with the upstream block name
# so the ratchet is visible: when rubyrs lands an Encoding
# story, un-comment the relevant block and re-run.

describe "String#length" do
  it "returns the length of self" do
    assert_eq("".length, 0)
    assert_eq("\x00".length, 1)
    assert_eq("one".length, 3)
    assert_eq("two".length, 3)
    assert_eq("three".length, 5)
    assert_eq("four".length, 4)
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
  # rubyrs's `String#length` is `chars.count` on the UTF-8
  # decoded view; invalid-byte handling diverges from CRuby's
  # "+1 per invalid byte" semantics.

  # Skipped — upstream shared/length.rb:46–49
  #   "adds 1 (and not 2) for a incomplete surrogate in UTF-16"
  # Needs `force_encoding("UTF-16LE"/"UTF-16BE")`.

  # Skipped — upstream shared/length.rb:51–54
  #   "adds 1 for a broken sequence in UTF-32"
  # Needs `force_encoding("UTF-32LE"/"UTF-32BE")`.
end
