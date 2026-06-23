# OpenSSL::Cipher AES-256-CBC with PKCS#7 padding (the pre-GCM
# MessageEncryptor mode). Encrypt/decrypt round-trips, the always-added
# pad block on block-aligned input, `padding = 0`, and bad-decrypt
# detection (wrong key → invalid padding → CipherError). Deterministic
# fixed key/iv. Runs under --features _openssl; CRuby's core openssl is
# the oracle.
require "openssl"

key = ["603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4"].pack("H*")
iv  = ["000102030405060708090a0b0c0d0e0f"].pack("H*")

# Padded round-trip.
c = OpenSSL::Cipher.new("aes-256-cbc")
c.encrypt; c.key = key; c.iv = iv
ct = c.update("hello world, cbc!") + c.final
p ct.unpack1("H*")
d = OpenSSL::Cipher.new("aes-256-cbc")
d.decrypt; d.key = key; d.iv = iv
p (d.update(ct) + d.final)

# Block-aligned plaintext still gets a full 16-byte pad block.
c2 = OpenSSL::Cipher.new("aes-256-cbc")
c2.encrypt; c2.key = key; c2.iv = iv
ct2 = c2.update("0123456789abcdef") + c2.final
p ct2.bytesize
d2 = OpenSSL::Cipher.new("aes-256-cbc")
d2.decrypt; d2.key = key; d2.iv = iv
p (d2.update(ct2) + d2.final)

# padding = 0 — caller supplies block-aligned data, no pad added/stripped.
c3 = OpenSSL::Cipher.new("aes-256-cbc")
c3.encrypt; c3.key = key; c3.iv = iv; c3.padding = 0
ct3 = c3.update("0123456789abcdef") + c3.final
p ct3.unpack1("H*")
d3 = OpenSSL::Cipher.new("aes-256-cbc")
d3.decrypt; d3.key = key; d3.iv = iv; d3.padding = 0
p (d3.update(ct3) + d3.final)

# Wrong key → PKCS#7 validation fails → CipherError.
bad = OpenSSL::Cipher.new("aes-256-cbc")
bad.decrypt
bad.key = "\x01".b * 32
bad.iv = iv
begin
  bad.update(ct)
  bad.final
  puts "NO RAISE"
rescue OpenSSL::Cipher::CipherError
  puts "CipherError"
end

p OpenSSL::Cipher.new("aes-256-cbc").iv_len
p OpenSSL::Cipher.new("aes-256-cbc").key_len
