# `require 'openssl'` / `require 'zlib'` — Tier 1 lenient stubs.
# rack-session's cookie + encryptor `require` both at module-load
# time but only call `OpenSSL::Cipher` / `OpenSSL::HMAC` /
# `Zlib::Deflate` from inside request-time methods. The lenient
# stub materialises the `OpenSSL` / `Zlib` Module constants so the
# require succeeds; real crypto / compression stays behind a
# future Tier-3 battery (ADR 0019 Part E `_openssl`).
#
# Discovery: P3 Sinatra spike — rack-session/lib/rack/session/
# cookie.rb:8-9 previously stopped the load at
# `LoadError: cannot load such file -- openssl` (then zlib).
#
# Only the parity-able surface is checked (require dedup + the
# constant being a Module). The feature-absent divergence (real
# OpenSSL/Zlib methods vs the stub's NoMethodError) is NOT
# parity-tested — CRuby has the real implementations.

# Shape 1: require returns true on first load, false after
# (CRuby loaded-features dedup; rubyrs mirrors it).
puts "openssl_first=#{require 'openssl'}"
puts "openssl_second=#{require 'openssl'}"
puts "zlib_first=#{require 'zlib'}"
puts "zlib_second=#{require 'zlib'}"

# Shape 2: the namespace constants resolve as Modules.
puts "openssl_module=#{OpenSSL.is_a?(Module)}"
puts "zlib_module=#{Zlib.is_a?(Module)}"

# Shape 3: defined? reports them as constants.
puts "openssl_defined=#{defined?(OpenSSL) ? 'constant' : 'nil'}"
puts "zlib_defined=#{defined?(Zlib) ? 'constant' : 'nil'}"
