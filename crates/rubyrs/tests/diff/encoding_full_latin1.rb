# `_encoding_full` v1 (ADR 0020 Tier 2, amended): the ISO-8859-1
# registry entry, hand-written — Latin-1 bytes ARE the first 256
# codepoints, so both transcoding directions are table-free and
# dodge the WHATWG latin1→windows-1252 label trap entirely.
# Covers: find (case-insensitive + ISO8859-1 alias), the constant,
# encode both ways (undef raise with CRuby's U+XXXX message,
# undef: :replace with default "?" and custom replace:),
# force_encoding, valid_encoding? (total — every byte valid),
# length/chars (single-byte units, tag-carrying), compatibility
# (+ with ASCII / CompatibilityError with non-ASCII UTF-8), and
# the \xNN inspect.
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0,70]}"; end; puts "#{l}: #{r}"; end
t("find")          { Encoding.find("ISO-8859-1") }
t("find lc")       { Encoding.find("iso-8859-1") }
t("find alias")    { Encoding.find("ISO8859-1") }
t("const")         { Encoding::ISO_8859_1 }
t("u2l basic")     { "héllo".encode("ISO-8859-1").bytes }
t("u2l enc")       { "héllo".encode("ISO-8859-1").encoding }
t("u2l undef")     { "日本".encode("ISO-8859-1") }
t("l2u roundtrip") { "héllo".encode("ISO-8859-1").encode("UTF-8") == "héllo" }
t("force")         { "\xe9".b.force_encoding("ISO-8859-1").encoding }
t("valid")         { "\xe9".b.force_encoding("ISO-8859-1").valid_encoding? }
t("length")        { "\xe9\xe8".b.force_encoding("ISO-8859-1").length }
t("chars")         { "\xe9".b.force_encoding("ISO-8859-1").chars }
t("eq vs utf8")    { "\xe9".b.force_encoding("ISO-8859-1") == "é" }
t("plus incompat") { "\xe9".b.force_encoding("ISO-8859-1") + "é" }
t("plus ascii")    { ("\xe9".b.force_encoding("ISO-8859-1") + "ab").encoding }
t("inspect")       { "\xe9ab".b.force_encoding("ISO-8859-1").inspect }
t("replace")       { "日x".encode("ISO-8859-1", undef: :replace).bytes }
t("replace custom"){ "日x".encode("ISO-8859-1", undef: :replace, replace: "_").bytes }
