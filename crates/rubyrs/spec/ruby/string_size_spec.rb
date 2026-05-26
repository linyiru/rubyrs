# Adapted from ruby/spec core/string/size_spec.rb at
# upstream commit 448cb340 (2026-05). The upstream file is
# `it_behaves_like :string_length, :size` against the same
# `shared/length.rb` — extractor v0.4 inlines the shared body
# with `@method` → `:size`, producing identical assertions to
# string_length_spec.rb but against `String#size`. Both
# consumers of the shared example ship as the first v0.4
# cross-file dogfood.
#   https://github.com/ruby/spec/blob/448cb340/core/string/size_spec.rb
#   https://github.com/ruby/spec/blob/448cb340/core/string/shared/length.rb
#
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

  # Skipped — upstream shared/length.rb:13–18 needs
  # `Encoding::UTF_32BE` / `Encoding::SHIFT_JIS` constants and
  # actual transcoding semantics. rubyrs has `String#encode` /
  # `String#force_encoding` as no-op stubs (see `vm/string.rs`),
  # but no `Encoding` constants and no real codepoint
  # re-encoding — strings stay UTF-8 bytes either way, so the
  # `.size == 400` assertion after `.encode(...)` would silently
  # pass on the no-op when in fact the round-trip isn't happening.
  #
  # it "returns the length of a string in different encodings" do
  #   utf8_str = 'こにちわ' * 100
  #   assert_eq(utf8_str.send(:size), 400)
  #   assert_eq(utf8_str.encode(Encoding::UTF_32BE).send(:size), 400)
  #   assert_eq(utf8_str.encode(Encoding::SHIFT_JIS).send(:size), 400)
  # end

  # Skipped — upstream shared/length.rb:20–25 needs
  # `+''` mutable-string literal and `String#force_encoding`.
  #
  # it "returns the length of the new self after encoding is changed" do
  #   str = +'こにちわ'
  #   str.send(:size)
  #
  #   assert_eq(str.force_encoding('BINARY').send(:size), 12)
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
  #   assert_eq(concat.send(:size), 2)
  #   concat.force_encoding(Encoding::ASCII_8BIT)
  #   assert_eq(concat.send(:size), 4)
  # end

  # Skipped — upstream shared/length.rb:40–44; rubyrs's
  # `String#size` is `chars.count` on the UTF-8 decoded
  # view; invalid-byte handling diverges from CRuby's "+1
  # per invalid byte" semantics.
  #
  # it "adds 1 for every invalid byte in UTF-8" do
  #   assert_eq("\xF4\x90\x80\x80".send(:size), 4)
  #   assert_eq("a\xF4\x90\x80\x80b".send(:size), 6)
  #   assert_eq("é\xF4\x90\x80\x80è".send(:size), 6)
  # end

  # Skipped — upstream shared/length.rb:46–49 needs
  # `force_encoding("UTF-16LE"/"UTF-16BE")`.
  #
  # it "adds 1 (and not 2) for an incomplete surrogate in UTF-16" do
  #   assert_eq("\x00\xd8".dup.force_encoding("UTF-16LE").send(:size), 1)
  #   assert_eq("\xd8\x00".dup.force_encoding("UTF-16BE").send(:size), 1)
  # end

  # Skipped — upstream shared/length.rb:51–54 needs
  # `force_encoding("UTF-32LE"/"UTF-32BE")`.
  #
  # it "adds 1 for a broken sequence in UTF-32" do
  #   assert_eq("\x04\x03\x02\x01".dup.force_encoding("UTF-32LE").send(:size), 1)
  #   assert_eq("\x01\x02\x03\x04".dup.force_encoding("UTF-32BE").send(:size), 1)
  # end
end
