# Marshal.load over REAL CRuby 4.8 bytes — the load-only
# common-tag subset (__rubyrs_marshal_load_binary): nil/bool/
# Fixnum (all length encodings) / Float / String (I-wrapped
# encoding ivars) / Symbol + symlink / Array / Hash. Byte
# samples are hardcoded CRuby `Marshal.dump` output so both
# runtimes load the same stream (our own dump is the Tier-1
# same-process token, deliberately not byte-compatible).
# Motivating consumer: addressable's pregenerated unicode.data
# (loaded via `File.open(path, "rb") { |f| Marshal.load(f.read) }`
# — which also pins the binary-mode whole-buffer read staying
# byte-transparent instead of U+FFFD-mangling).

p Marshal.load("\x04\x08\x30".b)
p Marshal.load("\x04\x08\x54".b)
p Marshal.load("\x04\x08\x69\x2F".b)
p Marshal.load("\x04\x08\x69\xFE\xD4\xFE".b)
p Marshal.load("\x04\x08\x69\x03\x70\x11\x01".b)
p Marshal.load("\x04\x08\x66\x08\x33\x2E\x35".b)
p Marshal.load("\x04\x08\x49\x22\x0A\x68\x65\x6C\x6C\x6F\x06\x3A\x06\x45\x54".b)
p Marshal.load("\x04\x08\x3A\x08\x66\x6F\x6F".b)
p Marshal.load("\x04\x08\x5B\x0A\x69\x06\x30\x49\x22\x06\x78\x06\x3A\x06\x45\x54\x3A\x06\x73\x3B\x06".b)
p Marshal.load("\x04\x08\x7B\x07\x69\x46\x5B\x08\x69\x00\x30\x69\x66\x49\x22\x06\x6B\x06\x3A\x06\x45\x54\x3A\x06\x76".b)
p Marshal.load("\x04\x08\x49\x22\x0B\x68\xC3\xA9\x6C\x6C\x6F\x06\x3A\x06\x45\x54".b)

# Garbage header fails loud too.
begin
  Marshal.load("not marshal at all")
rescue TypeError
  puts "garbage: TypeError"
end

# Binary-mode whole-buffer handle read stays byte-transparent:
# round-trip the hash sample through a temp file.
path = "/tmp/rubyrs-marshal-fixture-#{Process.pid}.bin"
File.binwrite(path, "\x04\x08\x7B\x07\x69\x46\x5B\x08\x69\x00\x30\x69\x66\x49\x22\x06\x6B\x06\x3A\x06\x45\x54\x3A\x06\x76".b)
h = File.open(path, "rb") { |f| Marshal.load(f.read) }
p h
File.delete(path)
