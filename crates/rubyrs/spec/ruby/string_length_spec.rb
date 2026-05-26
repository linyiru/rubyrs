# Adapted from ruby/spec core/string/length_spec.rb at
# upstream commit 448cb340 (2026-05). Pairs with the
# inlined `shared/length.rb` body at the same commit:
#   https://github.com/ruby/spec/blob/448cb340/core/string/length_spec.rb
#   https://github.com/ruby/spec/blob/448cb340/core/string/shared/length.rb
#
# First extractor v0.4 output — produced via:
#   rubyrs-spec-extract upstream/length_spec.rb \
#     --shared upstream/shared/length.rb
# which inlined the `it_behaves_like :string_length, :length`
# call against the shared body and substituted `@method` → `:length`.
#
# The shared body calls `"".send(@method)`, so v0.4 emits
# `"".send(:length)` verbatim. `Object#send` is now in subset
# (see `tests/diff/object_send.rb`), so the extractor output
# runs as-is — no hand-fix needed.
#
# Six of the upstream shared-body `it` blocks need features
# rubyrs doesn't model — Encoding objects (`Encoding::UTF_32BE`,
# `Encoding::SHIFT_JIS`), `String#encode`, `String#force_encoding`,
# `String#encoding`, and the "invalid byte adds 1" semantics.
# Those `it` blocks are preserved below as commented-out code
# (`@method` already substituted to `:length`, `.should ==`
# rewritten to `assert_eq` for symmetry with the live block)
# so a future un-skip is mechanical: when rubyrs lands the
# relevant feature, drop the leading `# ` and re-run.

describe "String#length" do
  it "returns the length of self" do
    assert_eq("".send(:length), 0)
    assert_eq("\x00".send(:length), 1)
    assert_eq("one".send(:length), 3)
    assert_eq("two".send(:length), 3)
    assert_eq("three".send(:length), 5)
    assert_eq("four".send(:length), 4)
  end

  # Skipped — upstream shared/length.rb:13–18 needs
  # `Encoding::UTF_32BE` / `Encoding::SHIFT_JIS` constants and
  # actual transcoding semantics. rubyrs has `String#encode` /
  # `String#force_encoding` as no-op stubs (see `vm/string.rs`),
  # but no `Encoding` constants and no real codepoint
  # re-encoding — strings stay UTF-8 bytes either way, so the
  # `.length == 400` assertion after `.encode(...)` would
  # silently pass on the no-op when in fact the round-trip
  # isn't happening.
  #
  # it "returns the length of a string in different encodings" do
  #   utf8_str = 'こにちわ' * 100
  #   assert_eq(utf8_str.send(:length), 400)
  #   assert_eq(utf8_str.encode(Encoding::UTF_32BE).send(:length), 400)
  #   assert_eq(utf8_str.encode(Encoding::SHIFT_JIS).send(:length), 400)
  # end

  # Skipped — upstream shared/length.rb:20–25 needs
  # `+''` mutable-string literal and `String#force_encoding`.
  #
  # it "returns the length of the new self after encoding is changed" do
  #   str = +'こにちわ'
  #   str.send(:length)
  #
  #   assert_eq(str.force_encoding('BINARY').send(:length), 12)
  # end

  # Skipped — upstream shared/length.rb:27–38 needs
  # `String#encoding` returning an Encoding object and
  # `force_encoding(Encoding::ASCII_8BIT)`.
  #
  # it "returns the correct length after force_encoding(BINARY)" do
  #   utf8 = "あ"
  #   ascii = "a"
  #   concat = utf8 + ascii
  #
  #   assert_eq(concat.encoding, Encoding::UTF_8)
  #   assert_eq(concat.bytesize, 4)
  #
  #   assert_eq(concat.send(:length), 2)
  #   concat.force_encoding(Encoding::ASCII_8BIT)
  #   assert_eq(concat.send(:length), 4)
  # end

  # Skipped — upstream shared/length.rb:40–44; rubyrs's
  # `String#length` is `chars.count` on the UTF-8 decoded
  # view; invalid-byte handling diverges from CRuby's "+1
  # per invalid byte" semantics.
  #
  # it "adds 1 for every invalid byte in UTF-8" do
  #   assert_eq("\xF4\x90\x80\x80".send(:length), 4)
  #   assert_eq("a\xF4\x90\x80\x80b".send(:length), 6)
  #   assert_eq("é\xF4\x90\x80\x80è".send(:length), 6)
  # end

  # Skipped — upstream shared/length.rb:46–49 needs
  # `force_encoding("UTF-16LE"/"UTF-16BE")`.
  #
  # it "adds 1 (and not 2) for an incomplete surrogate in UTF-16" do
  #   assert_eq("\x00\xd8".dup.force_encoding("UTF-16LE").send(:length), 1)
  #   assert_eq("\xd8\x00".dup.force_encoding("UTF-16BE").send(:length), 1)
  # end

  # Skipped — upstream shared/length.rb:51–54 needs
  # `force_encoding("UTF-32LE"/"UTF-32BE")`.
  #
  # it "adds 1 for a broken sequence in UTF-32" do
  #   assert_eq("\x04\x03\x02\x01".dup.force_encoding("UTF-32LE").send(:length), 1)
  #   assert_eq("\x01\x02\x03\x04".dup.force_encoding("UTF-32BE").send(:length), 1)
  # end
end
