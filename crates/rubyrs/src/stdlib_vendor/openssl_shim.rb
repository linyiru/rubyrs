# `openssl` — always-on constant shells for LOAD-TIME references.
#
# ActiveRecord 7's encryption module builds a rescue list at class-body
# load time (active_record/encryption/encryptor.rb:70):
#
#   DECRYPT_ERRORS = [OpenSSL::Cipher::CipherError, ...]
#
# so `require "active_record"` NameErrors unless the constant resolves.
# Hierarchy matches CRuby 3.4.1 exactly:
#
#   OpenSSL::Cipher::CipherError < OpenSSL::OpenSSLError < StandardError
#
# and `OpenSSL::Cipher` is a Class (not a Module). The `_openssl`
# battery (when compiled in) defines the REAL classes at boot — the
# `defined?` guards make this shim a no-op there. Without the battery
# these are constant shells only: usable in rescue clauses / arrays /
# `is_a?` checks; calling cipher METHODS still raises NoMethodError
# (ADR 0017 feature-absent surface — an app that actually encrypts
# fails loudly rather than silently).
module OpenSSL
  unless defined?(OpenSSL::OpenSSLError)
    class OpenSSLError < StandardError; end
  end
  unless defined?(OpenSSL::Cipher)
    class Cipher
      class CipherError < OpenSSL::OpenSSLError; end
    end
  end
end
