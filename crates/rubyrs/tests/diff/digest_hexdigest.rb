# Native SHA-256 / SHA-512 / SHA-1 / MD5 via Digest::SHA2 / SHA256 /
# SHA512 / SHA1 / MD5, byte-exact with CRuby's digest C extension.
# Class shortcut + the incremental new/update/hexdigest surface.
require "digest"
p Digest::SHA2.hexdigest("hello")
p Digest::SHA256.hexdigest("hello")
p Digest::SHA512.hexdigest("hello")
p Digest::SHA512.hexdigest("")
p Digest::SHA384.hexdigest("hello")
p Digest::SHA384.hexdigest("")
p Digest::SHA1.hexdigest("hello")
p Digest::MD5.hexdigest("abc")
p Digest::SHA256.hexdigest("")
p Digest::MD5.hexdigest("The quick brown fox jumps over the lazy dog")
d = Digest::SHA256.new
d.update("hel"); d << "lo"
p d.hexdigest
p (d.hexdigest == Digest::SHA256.hexdigest("hello"))
# digest of a longer multi-block input
p Digest::SHA256.hexdigest("a" * 1000)
# raw digest bytes are ASCII-8BIT (binary), not UTF-8
p Digest::SHA256.digest("abc").encoding.to_s
p Digest::SHA512.digest("abc").encoding.to_s
