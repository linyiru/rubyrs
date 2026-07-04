# String#encode's error surface, pinned against CRuby 3.4.1:
#
# - an unresolvable TARGET NAME is Encoding::ConverterNotFoundError
#   "code converter not found (SRC to DST)" — NOT ArgumentError (that
#   shape belongs to force_encoding). The SRC half names the
#   receiver's encoding, so all three core tags are pinned.
# - a non-String/non-Encoding argument is TypeError "no implicit
#   conversion of X into String" for BOTH encode and force_encoding.
#
# This same ConverterNotFoundError path is what a default (non-
# `_encoding_full`) build raises for name-registered constants with
# no converter (e.g. "x".encode("GB18030")); CRuby CAN convert those,
# so that instance is pinned by the golden fixture
# tests/fixtures/encoding_named_no_converter.rb instead.

def t(l)
  yield
  puts "#{l}: no error (bug)"
rescue => e
  puts "#{l}: #{e.class}: #{e.message}"
end

t("utf8 src")   { "x".encode("NOPE-ENC") }
t("binary src") { "x".b.encode("NOPE-ENC") }
t("ascii src")  { "x".dup.force_encoding("US-ASCII").encode("NOPE-ENC") }
t("with opts")  { "x".encode("NOPE-ENC", undef: :replace) }
t("encode int") { "x".encode(123) }
t("encode sym") { "x".encode(:nope) }
t("fe unknown") { "x".dup.force_encoding("NOPE-ENC") }
t("fe int")     { "x".dup.force_encoding(123) }
t("fe sym")     { "x".dup.force_encoding(:nope) }

# The error class is rescuable under its CRuby ancestry
# (EncodingError < StandardError).
begin
  "x".encode("NOPE-ENC")
rescue EncodingError => e
  puts "rescued as EncodingError: #{e.class}"
end
