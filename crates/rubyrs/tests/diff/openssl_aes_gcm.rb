# OpenSSL::Cipher AES-256-GCM (authenticated encryption) — what
# ActiveSupport::MessageEncryptor / Rails 7 credentials use. Encrypt
# captures #auth_tag; decrypt verifies it (raising CipherError on a
# tampered tag). Deterministic fixed key/iv/aad (GCM spec test case 16)
# so the output is stable. Runs under --features _openssl; CRuby's core
# openssl is the oracle.
require "openssl"

key = ["feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308"].pack("H*")
iv  = ["cafebabefacedbaddecaf888"].pack("H*")
aad = ["feedfacedeadbeeffeedfacedeadbeefabaddad2"].pack("H*")
pt  = ["d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39"].pack("H*")

# Encrypt → ciphertext + 16-byte tag.
c = OpenSSL::Cipher.new("aes-256-gcm")
c.encrypt
c.key = key
c.iv = iv
c.auth_data = aad
ct = c.update(pt) + c.final
tag = c.auth_tag
p ct.unpack1("H*")
p tag.unpack1("H*")
p tag.bytesize

# Decrypt with the right tag → original plaintext.
d = OpenSSL::Cipher.new("aes-256-gcm")
d.decrypt
d.key = key
d.iv = iv
d.auth_data = aad
d.auth_tag = tag
p (d.update(ct) + d.final) == pt

# Tampered tag → CipherError.
bad = OpenSSL::Cipher.new("aes-256-gcm")
bad.decrypt
bad.key = key
bad.iv = iv
bad.auth_data = aad
bad.auth_tag = "\x00".b * 16
begin
  bad.update(ct)
  bad.final
  puts "NO RAISE"
rescue OpenSSL::Cipher::CipherError
  puts "CipherError"
end

# Wrong AAD → also fails authentication.
wrong = OpenSSL::Cipher.new("aes-256-gcm")
wrong.decrypt
wrong.key = key
wrong.iv = iv
wrong.auth_data = "different aad".b
wrong.auth_tag = tag
begin
  wrong.update(ct)
  wrong.final
  puts "NO RAISE"
rescue OpenSSL::Cipher::CipherError
  puts "CipherError"
end

# Empty plaintext + empty AAD (GCM test case 14 shape) round-trips.
e = OpenSSL::Cipher.new("aes-256-gcm")
e.encrypt
e.key = "\x00".b * 32
e.iv = "\x00".b * 12
empty_ct = e.update("") + e.final
p empty_ct.empty?
p e.auth_tag.unpack1("H*")

p OpenSSL::Cipher.new("aes-256-gcm").iv_len
p OpenSSL::Cipher.new("aes-256-gcm").key_len

# authenticated? — true for GCM, false for CTR (ActiveSupport's aead_mode?).
p OpenSSL::Cipher.new("aes-256-gcm").authenticated?
p OpenSSL::Cipher.new("aes-256-ctr").authenticated?

# iv_len= — AEAD IV-length control. rack-session 2.x sets `iv_len = 12`
# before `random_iv` when encrypting session cookies; without it the
# encryptor raised NoMethodError. The set value is reflected by the getter,
# and a non-default length still round-trips through the GCM core.
g = OpenSSL::Cipher.new("aes-256-gcm")
g.iv_len = 7
p g.iv_len                                   # 7
ge = OpenSSL::Cipher.new("aes-256-gcm")
ge.encrypt
ge.key = key
ge.iv_len = 12                               # the rack-session pattern
ge.iv = iv                                   # fixed 12-byte iv for determinism
ge.auth_data = aad
ct2 = ge.update(pt) + ge.final
tag2 = ge.auth_tag
p ct2.unpack1("H*") == ct.unpack1("H*")      # same as the iv_len-default run
p tag2.unpack1("H*") == tag.unpack1("H*")

# iv_len= on a non-AEAD cipher raises CipherError (CRuby parity).
begin
  OpenSSL::Cipher.new("aes-256-ctr").iv_len = 12
  puts "NO RAISE"
rescue OpenSSL::Cipher::CipherError
  puts "CipherError"
end
