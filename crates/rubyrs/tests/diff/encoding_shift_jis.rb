# Strict Shift_JIS as a distinct Encoding from Windows-31J. Shares the
# WHATWG shift_jis transcoder; the common plane (kana + JIS X 0208
# kanji) round-trips identically.
p Encoding.find("Shift_JIS").name
p Encoding.find("SJIS").name        # alias of Windows-31J (CRuby)
p Encoding::Shift_JIS.name
s = "ふがぴょん日本語"
e = s.encode("Shift_JIS")
p e.encoding.to_s
p e.bytes
p e.encode("UTF-8")
p e.encode(Encoding::UTF_8) == s
# magic-comment-style force then re-decode
e2 = "テスト".encode("Shift_JIS")
p e2.encode("UTF-8")
