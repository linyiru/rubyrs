# `require "openssl"` exposes the OpenSSL::Cipher::CipherError constant
# with CRuby's exact hierarchy (CipherError < OpenSSLError <
# StandardError; Cipher is a Class). ActiveRecord 7's encryption module
# references it in a class-body rescue list (encryptor.rb:70
# `DECRYPT_ERRORS = [OpenSSL::Cipher::CipherError, ...]`), so
# `require "active_record"` NameErrors without it. In the default build
# these are constant SHELLS (real crypto lives behind `_openssl`);
# everything pinned here holds for both.
require "openssl"

p defined?(OpenSSL::Cipher::CipherError)
p OpenSSL::Cipher.instance_of?(Class)
p OpenSSL::Cipher::CipherError < OpenSSL::OpenSSLError
p OpenSSL::Cipher::CipherError < StandardError
p OpenSSL::OpenSSLError < StandardError

# The ActiveRecord shape: a rescue list built at class-body load,
# splatted in a rescue clause.
DECRYPT_ERRORS = [OpenSSL::Cipher::CipherError, ArgumentError]
begin
  raise ArgumentError, "boom"
rescue *DECRYPT_ERRORS => e
  puts "rescued: #{e.class}"
end
