# `_encoding_full` v2: the seven encoding_rs-backed registry
# entries (Windows-1252, ISO-8859-15, KOI8-R, Windows-31J, EUC-JP,
# GBK, Big5) — naming follows CRuby with the WHATWG mapping
# documented in encoding_full.rs: the WHATWG shift_jis table IS
# windows-31j semantics, so it registers under Windows-31J (with
# CRuby's SJIS/CP932 aliases) and CRuby's strict Shift_JIS is
# deliberately absent; WHATWG big5 covers the common plane pinned
# here. Multi-byte length/chars/valid_encoding?, the \x{XXXX}
# per-char inspect, byteslice, and undef/replace behaviour all
# compared against CRuby.
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0,60]}"; end; puts "#{l}: #{r}"; end
t("sjis alias SJIS"){ Encoding.find("SJIS") }
t("w31j circled")   { "①".encode("Windows-31J").bytes }
t("eucjp basic")    { "日本語".encode("EUC-JP").bytes }
t("eucjp round")    { "日本語".encode("EUC-JP").encode("UTF-8") == "日本語" }
t("gbk")            { "中文".encode("GBK").bytes }
t("gbk round")      { "中文".encode("GBK").encode("UTF-8") == "中文" }
t("big5")           { "中文".encode("Big5").bytes }
t("koi8r")          { "русский".encode("KOI8-R").bytes.first(3) }
t("w1252 euro")     { "€".encode("Windows-1252").bytes }
t("w1252 80 dec")   { "\x80".dup.force_encoding("Windows-1252").encode("UTF-8") }
t("latin9 euro")    { "€".encode("ISO-8859-15").bytes }
t("latin9 a4 dec")  { "\xa4".dup.force_encoding("ISO-8859-15").encode("UTF-8") }
t("latin1 a4 dec")  { "\xa4".dup.force_encoding("ISO-8859-1").encode("UTF-8") }
t("w31j len/chars") { s = "日本語".encode("Windows-31J"); [s.length, s.chars.map(&:bytes)] }
t("w31j cut valid") { "日本語".encode("Windows-31J").byteslice(0,1).valid_encoding? }
t("eucjp refl")     { "日".encode("EUC-JP").encoding }
t("gbk refl")       { "中".encode("GBK").encoding }
t("big5 refl")      { "中".encode("Big5").encoding }
t("koi8 refl")      { "д".encode("KOI8-R").encoding }
t("w1252 refl")     { "€".encode("Windows-1252").encoding }
t("l9 refl")        { "€".encode("ISO-8859-15").encoding }
t("gbk undef")      { "русский".encode("GBK") }
t("roundtrips")     { %w[Windows-31J EUC-JP GBK Big5].map { |e| "中日".encode(e).encode("UTF-8") == "中日" rescue false } }

t("byteslice")      { ["hello".byteslice(1,3), "hello".byteslice(1), "hello".byteslice(-2,2), "x".byteslice(5)] }
t("byteslice enc")  { "日本".encode("Windows-31J").byteslice(0,2).encoding }
t("w1252 round")    { "café€".encode("Windows-1252").encode("UTF-8") == "café€" }
t("koi8 round")     { "русский".encode("KOI8-R").encode("UTF-8") == "русский" }
t("l9 round")       { "œuvre€".encode("ISO-8859-15").encode("UTF-8") == "œuvre€" }
t("gbk len/chars")  { s = "中文a".encode("GBK"); [s.length, s.chars.map(&:bytes)] }
t("big5 inspect")   { "中".encode("Big5").inspect }
t("sjis replace")   { "①x".encode("SJIS", undef: :replace).bytes }
