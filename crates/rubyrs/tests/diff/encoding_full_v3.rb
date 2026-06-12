# `_encoding_full` v3 (E2 close-out): the reflection trio
# (Encoding.list / name_list / aliases — asserted by intersection,
# since CRuby's registry is ~10x ours), Unicode case ops on
# registry-tagged strings (full mapping: latin1 ß.upcase grows to
# "SS"; unmappable results keep their original bytes — ÿ.upcase
# stays ÿ), and the Other↔Binary/US-ASCII encode pairs with
# CRuby's UTF-8-pivot-chain error wording.
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0,70]}"; end; puts "#{l}: #{r}"; end

OURS = %w[ASCII-8BIT Big5 EUC-JP GBK ISO-8859-1 ISO-8859-15 KOI8-R
          US-ASCII UTF-8 Windows-1252 Windows-31J].freeze
t("list covers")  { (Encoding.list.map(&:name) & OURS).sort == OURS.sort }
t("list types")   { Encoding.list.all? { |e| e.is_a?(Encoding) } }
t("name_list")    { (Encoding.name_list & (OURS + %w[SJIS CP932 BINARY ASCII])).sort }
OUR_ALIASES = %w[ASCII BINARY CP1252 CP932 CP936 ISO8859-1 ISO8859-15 SJIS].freeze
t("aliases")      { Encoding.aliases.select { |k, _| OUR_ALIASES.include?(k) }.sort.to_h }

latin = "caf\xE9 \xC0bc".dup.force_encoding("ISO-8859-1")
t("up bytes")     { latin.upcase.bytes }
t("up enc")       { latin.upcase.encoding }
t("down bytes")   { latin.downcase.bytes }
t("sharp-s")      { "stra\xDFe".dup.force_encoding("ISO-8859-1").upcase.bytes }
t("unmappable")   { "\xFF".dup.force_encoding("ISO-8859-1").upcase.bytes }
t("capitalize")   { "\xE9BC".dup.force_encoding("ISO-8859-1").capitalize.bytes }
t("swapcase")     { "aB\xE9\xC9".dup.force_encoding("ISO-8859-1").swapcase.bytes }
t("w31j ascii")   { "abc".encode("Windows-31J").upcase.bytes }

l = "caf\xE9".dup.force_encoding("ISO-8859-1")
t("o2b")          { l.encode("ASCII-8BIT") }
t("o2a")          { l.encode("US-ASCII") }
t("o2a ok")       { "cafe".dup.force_encoding("ISO-8859-1").encode("US-ASCII").encoding }
t("b2o")          { "caf\xE9".b.encode("ISO-8859-1") }
t("b2o ok")       { "cafe".b.encode("ISO-8859-1").encoding }
t("a2o")          { "cafe".dup.force_encoding("US-ASCII").encode("ISO-8859-1").encoding }
t("a2mb")         { "cafe".dup.force_encoding("US-ASCII").encode("Windows-31J").encoding }

# E3: registry tags + the ext:int transcoding read form.
path = "/tmp/rubyrs-e3-full-#{Process.pid}.bin"
File.binwrite(path, "caf\xE9\n".b)
t("read l1 tag")  { s = File.read(path, encoding: "ISO-8859-1"); [s.bytes, s.encoding.name, s.valid_encoding?] }
t("read l1>u8")   { s = File.read(path, encoding: "ISO-8859-1:UTF-8"); [s.bytes, s.encoding.name] }
t("mode r:l1")    { File.open(path, "r:ISO-8859-1") { |f| f.read }.encoding.name }
t("mode r:l1:u8") { File.open(path, "r:ISO-8859-1:UTF-8") { |f| f.read }.bytes }
t("default l1")   { Encoding.default_external = "ISO-8859-1"; e = File.read(path).encoding.name; Encoding.default_external = "UTF-8"; e }
t("u8>l1 file")   { File.binwrite(path, "caf\xC3\xA9".b); s = File.read(path, encoding: "UTF-8:ISO-8859-1"); [s.bytes, s.encoding.name] }
t("bad seq")      { File.binwrite(path, "\xFF\xFE".b); File.read(path, encoding: "UTF-8:ISO-8859-1") }
File.delete(path)
