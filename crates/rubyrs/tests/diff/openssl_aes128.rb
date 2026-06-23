# AES-128 across the three OpenSSL::Cipher modes (the generalized key
# schedule: 16-byte key, 10 rounds). Deterministic fixed key/iv. Runs
# under --features _openssl; CRuby's core openssl is the oracle.
require "openssl"

key = ["000102030405060708090a0b0c0d0e0f"].pack("H*")  # 16 bytes
iv  = ["101112131415161718191a1b1c1d1e1f"].pack("H*")  # 16 bytes
zero_iv = "\x00".b * 16

p OpenSSL::Cipher.new("aes-128-cbc").key_len   # 16
p OpenSSL::Cipher.new("aes-128-gcm").key_len   # 16
p OpenSSL::Cipher.new("aes-128-gcm").iv_len    # 12

# CBC with padding=0 on a single FIPS-197 C.1 block (iv=0 ⇒ raw block).
ecb = OpenSSL::Cipher.new("aes-128-cbc")
ecb.encrypt; ecb.key = key; ecb.iv = zero_iv; ecb.padding = 0
block = ["00112233445566778899aabbccddeeff"].pack("H*")
p (ecb.update(block) + ecb.final).unpack1("H*")  # 69c4e0d8...

# CBC padded round-trip.
c = OpenSSL::Cipher.new("aes-128-cbc"); c.encrypt; c.key = key; c.iv = iv
ct = c.update("hello aes-128 cbc!") + c.final
p ct.unpack1("H*")
d = OpenSSL::Cipher.new("aes-128-cbc"); d.decrypt; d.key = key; d.iv = iv
p (d.update(ct) + d.final)

# CTR round-trip.
ctr = OpenSSL::Cipher.new("aes-128-ctr"); ctr.encrypt; ctr.key = key; ctr.iv = iv
sc = ctr.update("stream cipher 128")
p sc.unpack1("H*")
ctr2 = OpenSSL::Cipher.new("aes-128-ctr"); ctr2.decrypt; ctr2.key = key; ctr2.iv = iv
p ctr2.update(sc)

# GCM with a fixed 12-byte IV.
giv = ["202122232425262728292a2b"].pack("H*")
g = OpenSSL::Cipher.new("aes-128-gcm"); g.encrypt; g.key = key; g.iv = giv; g.auth_data = "hdr"
gc = g.update("hello aes-128 gcm") + g.final
tag = g.auth_tag
p gc.unpack1("H*")
p tag.unpack1("H*")
gd = OpenSSL::Cipher.new("aes-128-gcm"); gd.decrypt; gd.key = key; gd.iv = giv; gd.auth_data = "hdr"; gd.auth_tag = tag
p (gd.update(gc) + gd.final)

# Wrong-size key → ArgumentError (CRuby class).
x = OpenSSL::Cipher.new("aes-128-cbc"); x.encrypt
begin
  x.key = "\x00".b * 32
rescue => e
  puts "#{e.class}: #{e.message}"
end
