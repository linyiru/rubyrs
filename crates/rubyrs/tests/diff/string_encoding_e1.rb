# E1 encoding semantics (ADR 0020, first semantic slice): the
# encoding TAG drives reflection. CRuby ground truth pinned here:
# String#encoding returns the singleton (BINARY renders with the
# dual-name inspect), force_encoding flips the tag in place
# (case-insensitive names, Encoding objects, ArgumentError on
# unknown, FrozenError on frozen receivers), dup/clone/+@/-@ carry
# the tag, valid_encoding? judges bytes AGAINST the tag, and
# encode's E1 subset copies within-encoding / ASCII-only and raises
# Encoding::UndefinedConversionError (CRuby's class and message
# shape) where real transcoding would be needed.
#
# == / + cross-encoding compatibility is the NEXT slice — not
# asserted here.
def t(label)
  r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0,60]}"; end
  puts "#{label}: #{r}"
end
t("lit enc")        { "x".encoding }
t("b enc")          { "x".b.encoding }
t("b dup enc")      { "x".b.dup.encoding }
t("b clone enc")    { "x".b.clone.encoding }
t("force bin")      { "x".dup.force_encoding("ASCII-8BIT").encoding }
t("force BINARY")   { "x".dup.force_encoding("BINARY").encoding }
t("force binary lc"){ "x".dup.force_encoding("binary").encoding }
t("force utf8 lc")  { "x".b.force_encoding("utf-8").encoding }
t("force us-ascii") { "x".dup.force_encoding("US-ASCII").encoding }
t("force enc obj")  { "x".dup.force_encoding(Encoding::ASCII_8BIT).encoding }
t("force unknown")  { "x".dup.force_encoding("nope") }
t("force ret self") { s = "x".dup; s.force_encoding("BINARY").equal?(s) }
t("valid utf8")     { "héllo".valid_encoding? }
t("valid bad utf8") { "\xff\xfe".dup.force_encoding("UTF-8").valid_encoding? }
t("valid bin")      { "\xff".b.valid_encoding? }
t("enc==")          { "x".encoding == "y".encoding }
t("ascii_only utf") { "abc".ascii_only? }
t("ascii_only é")   { "é".ascii_only? }
t("ascii_only bin") { "abc".b.ascii_only? }
t("encode same")    { s="x"; e=s.encode("UTF-8"); [e, e.equal?(s), e.encoding] }
t("encode noargs")  { "x".b.encode.encoding }
t("encode b2u asc") { "abc".b.encode("UTF-8").encoding }
t("encode b2u bad") { "\xff".b.encode("UTF-8") }
t("encode u2b asc") { "abc".encode("ASCII-8BIT").encoding }
t("encode u2b bad") { "é".encode("ASCII-8BIT") }
t("plus mixed asc") { ("abc" + "def".b).encoding }
t("eq cross asc")   { "abc" == "abc".b }
