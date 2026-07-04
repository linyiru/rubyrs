# rubyrs-only golden pin (CANNOT be CRuby-diffed: CRuby ships real
# converters for these encodings and would succeed). In a build
# without `_encoding_full`, the name-registered constant family
# (Encoding::GB18030 etc. — required by activesupport at load time)
# exists but has NO transcoder, so String#encode declines with the
# SAME class + message shape CRuby uses when a converter is missing:
# Encoding::ConverterNotFoundError "code converter not found (SRC to
# DST)". force_encoding keeps its ArgumentError decline (there's no
# tag to flip to), and Encoding.find succeeds — the constant is real.
#
# NOTE for a future `_encoding_full`-gated test run: this fixture is
# only registered for the default suite (integration.rs); under
# `_encoding_full` these conversions succeed and the golden file
# would not match.

def t(l)
  r = yield
  puts "#{l}: ok #{r.inspect}"
rescue => e
  puts "#{l}: #{e.class}: #{e.message}"
end

t("encode name")   { "x".encode("GB18030") }
t("encode const")  { "x".encode(Encoding::GB18030) }
t("encode big5")   { "café".encode("Big5") }
t("encode utf16")  { "x".encode("UTF-16LE") }
t("encode!")       { "x".dup.encode!("GB18030") }
t("force")         { "x".dup.force_encoding("GB18030") }
t("find works")    { Encoding.find("GB18030").name }
