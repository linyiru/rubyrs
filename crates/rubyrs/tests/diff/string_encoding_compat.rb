# E1 encoding semantics, slice 2: cross-encoding COMPATIBILITY.
# CRuby ground truth: == / eql? need byte equality AND tag
# compatibility (pure-ASCII content is encoding-blind); hash is
# tag-sensitive exactly when content is non-ASCII (so ==-equal
# implies hash-equal); + / << / concat take the compatible side's
# encoding (receiver wins when both are ASCII-only; << / concat
# UPGRADE the receiver's tag) and raise
# Encoding::CompatibilityError when both sides are non-ASCII with
# different encodings; <=> breaks byte-equal ties by encoding
# index (BINARY=0 < UTF-8=1 < US-ASCII=2); interpolation compiles
# to the same + sequence and inherits all of it; uniq/Hash keys
# follow eql?+hash.
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0,70]}"; end; puts "#{l}: #{r}"; end
t("plus both-ascii")   { ("abc" + "def".b).encoding }
t("plus asc+bin")      { ("abc" + "\xff".b).encoding }
t("plus bin+asc")      { ("é".b + "abc").encoding }
t("plus empty+bin")    { ("" + "\xff".b).encoding }
t("plus incompat")     { "é" + "é".b }
t("shovel asc<<bin")   { a = +"abc"; a << "\xff".b; a.encoding }
t("shovel bin<<utf")   { a = "\xff".b.dup; a << "é"; a.encoding }
t("shovel incompat")   { a = +"é"; a << "\xff".b; a.encoding }
t("concat incompat")   { (+"é").concat("\xff".b) }
t("concat ok tag")     { a = +"abc"; a.concat("\xff".b); a.encoding }
t("eq asc")            { ["abc" == "abc".b, "abc".eql?("abc".b)] }
t("eq nonascii")       { ["é" == "é".b, "é".eql?("é".b)] }
t("hash asc")          { "abc".hash == "abc".b.hash }
t("hash nonascii")     { "é".hash == "é".b.hash }
t("hashkey asc")       { {"abc" => 1}["abc".b] }
t("hashkey nonascii")  { {"é" => 1}["é".b] }
t("cmp tags")          { ["é" <=> "é".b, "abc" <=> "abc".b, "é".b <=> "é"] }
t("usascii eq")        { "abc" == "abc".dup.force_encoding("US-ASCII") }
t("interp bin")        { "x#{"\xff".b}".encoding }
t("interp incompat")   { "é#{"\xff".b}" }
t("uniq cross")        { ["abc", "abc".b, "é", "é".b].uniq.size }
