# OpenSSL crypto surface beyond the SHA256/AES-256-CTR session core:
# generalized HMAC (RFC 2104 over any digest), PBKDF2 (KDF + PKCS5),
# constant-time compare, and OpenSSL::Digest streaming + class-method
# arity. SHA512 / AES-GCM / AES-CBC need native code and are out of
# scope here. Runs under --features _openssl; CRuby's core openssl is
# the oracle (loadable under --disable-gems).
require "openssl"

# --- Digest: one-shot, class-method arity, streaming ---
p OpenSSL::Digest::SHA256.hexdigest("abc")
p OpenSSL::Digest::SHA1.hexdigest("abc")
p OpenSSL::Digest.new("SHA1").hexdigest("abc")
p OpenSSL::Digest.digest("SHA256", "abc").unpack1("H*")
p OpenSSL::Digest.hexdigest("SHA1", "abc")
d = OpenSSL::Digest::SHA256.new
d.update("a"); d << "bc"
p d.hexdigest
p OpenSSL::Digest::SHA256.new.digest_length

# --- SHA-512 / SHA-384 (64-bit-word digests) ---
p OpenSSL::Digest::SHA512.hexdigest("abc")
p OpenSSL::Digest.new("SHA512").hexdigest("")
p OpenSSL::Digest::SHA512.new.digest_length
p OpenSSL::Digest::SHA384.hexdigest("abc")
p OpenSSL::Digest::SHA384.new.digest_length

# --- HMAC across algorithms ---
p OpenSSL::HMAC.hexdigest("SHA256", "key", "data")
p OpenSSL::HMAC.hexdigest("SHA1", "key", "data")
p OpenSSL::HMAC.hexdigest("SHA384", "key", "data")
p OpenSSL::HMAC.hexdigest("SHA512", "key", "data")
p OpenSSL::HMAC.hexdigest("MD5", "key", "data")
p OpenSSL::HMAC.hexdigest("SHA256", "k" * 100, "data")  # key longer than block
p OpenSSL::HMAC.hexdigest("SHA512", "k" * 200, "data")  # key longer than 128-byte block
p OpenSSL::HMAC.hexdigest(OpenSSL::Digest.new("SHA1"), "key", "data")  # digest-object arg

# --- PBKDF2 (RFC 2898) ---
p OpenSSL::KDF.pbkdf2_hmac("password", salt: "salt", iterations: 1000, length: 32, hash: "SHA256").unpack1("H*")
p OpenSSL::KDF.pbkdf2_hmac("password", salt: "salt", iterations: 2048, length: 20, hash: "SHA1").unpack1("H*")
p OpenSSL::KDF.pbkdf2_hmac("password", salt: "salt", iterations: 1000, length: 64, hash: "SHA512").unpack1("H*")
p OpenSSL::PKCS5.pbkdf2_hmac("pw", "salt", 1000, 32, OpenSSL::Digest.new("SHA256")).unpack1("H*")
p OpenSSL::PKCS5.pbkdf2_hmac_sha1("pw", "salt", 1000, 20).unpack1("H*")

# --- constant-time comparison ---
p OpenSSL.fixed_length_secure_compare("abcdef", "abcdef")
p OpenSSL.fixed_length_secure_compare("abcdef", "abcdeX")
begin
  OpenSSL.fixed_length_secure_compare("ab", "abc")
rescue => e
  puts "#{e.class}: #{e.message}"
end
p OpenSSL.secure_compare("abcdef", "abcdef")
p OpenSSL.secure_compare("ab", "abc")

# --- asymmetric-algorithm load surface (constant shells + version) ---
# Gems that SUPPORT RSA/EC reference these at load even when only the
# symmetric path is used (e.g. jwt's JWK). Values that match CRuby
# byte-for-byte: defined? is "constant"; the version number clears the
# 1.0.0 floor on any real/modern provider.
p defined?(OpenSSL::PKey::EC)
p defined?(OpenSSL::PKey::RSA)
p defined?(OpenSSL::PKey::PKeyError)
p OpenSSL::OPENSSL_VERSION_NUMBER >= 0x10000000
