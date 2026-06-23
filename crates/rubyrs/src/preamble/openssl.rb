# `_openssl` battery — pure-Ruby `OpenSSL::SSL::SSLSocket` veneer over the
# `__rubyrs_openssl_*` host fns (ADR 0029). The minimal TLS-client slice
# Net::HTTP https drives; single-layer discipline (host fns internal,
# these classes are the surface; bare `require "openssl"`, MRI-shape).
#
# Only `Cipher`/`PKey`/`X509`/`HMAC`/server-TLS are deferred (no consumer
# on the net/http path); reaching for them is a NameError in v1.

# net/http's https setup does `case @address when Resolv::IPv4::Regex,
# Resolv::IPv6::Regex` to skip SNI for literal IPs. The `resolv` require
# is a lenient stub (the battery does its own host-side DNS), so provide
# just the two address-classification regexps net/http references. Loose
# but correct for "is this an IP?" — real hostnames don't match.
unless defined?(Resolv::IPv4::Regex)
  module Resolv
    module IPv4
      Regex = /\A(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\z/
    end
    module IPv6
      Regex = /\A(?:[0-9A-Fa-f]{0,4}:){2,7}[0-9A-Fa-f]{0,4}\z/
    end
  end
end

module OpenSSL
  class OpenSSLError < StandardError; end

  # Version constants gems probe for capability/quirk detection (e.g.
  # jwt's `openssl_3?`). rubyrs's crypto is a pure-Rust reimplementation,
  # not a libssl binding, but it behaves like a modern (3.x) provider —
  # report that so version gates pick the current code paths.
  VERSION = "3.0.0"
  OPENSSL_VERSION = "OpenSSL 3.0.0 (rubyrs)"
  OPENSSL_VERSION_NUMBER = 0x30000000
  OPENSSL_LIBRARY_VERSION = "rubyrs pure-Rust crypto"

  # `OpenSSL::PKey` — constant shells only. The asymmetric algorithms
  # (RSA/EC/DSA sign/verify) aren't implemented, but gems that support
  # them reference these constants at load time (e.g. jwt's JWK builds
  # `[KTY, OpenSSL::PKey::EC]` key-type dispatch arrays). The symmetric
  # path (HMAC / AES) never touches them; calling a method here is a
  # NameError/NoMethodError, matching the "feature-absent surface".
  module PKey
    class PKeyError < OpenSSL::OpenSSLError; end
    class PKey; end
    class RSA < PKey; end
    class EC < PKey; end
    class DSA < PKey; end
  end

  # `OpenSSL::Random.random_bytes(n)` — n cryptographically-random bytes
  # as a binary (ASCII-8BIT) String. Backed by the same CSPRNG as
  # `SecureRandom` (the host RNG); the two are interchangeable for
  # callers that just want entropy. bcrypt's `generate_salt` uses this
  # to seed a 16-byte salt.
  module Random
    def self.random_bytes(n)
      SecureRandom.random_bytes(n)
    end
  end

  # `OpenSSL::Digest` — the symmetric-crypto slice Rack 3's session
  # `Encryptor` drives (`OpenSSL::Digest::SHA256.new`, handed to
  # `OpenSSL::HMAC.digest`). A Digest instance here is a thin name +
  # one-shot compute wrapper over the native `Digest` battery; HMAC and
  # the message-digest API both key off `#name`. Only SHA-256 has a
  # native HMAC path (all Rack needs); SHA1/SHA512/MD5 compute digests
  # but raise from HMAC.
  class Digest
    attr_reader :name

    # Algorithm name (CRuby's upcased form) → the lowercase tag the
    # native `RubyrsDigest` primitive understands, and the digest byte
    # length. Backed directly by the native digest so OpenSSL::Digest
    # does NOT depend on the `digest` stdlib being required first.
    TAGS = { "SHA256" => "sha256", "SHA2" => "sha256", "SHA1" => "sha1",
             "MD5" => "md5", "SHA512" => "sha512", "SHA384" => "sha384" }.freeze
    LENS = { "SHA256" => 32, "SHA2" => 32, "SHA1" => 20,
             "MD5" => 16, "SHA512" => 64, "SHA384" => 48 }.freeze

    def initialize(name)
      @name = name.to_s.upcase
      @buffer = "".b
    end

    # Base class methods take (name, data); each concrete subclass
    # overrides them to the (data) form below.
    def self.digest(name, data); new(name).digest(data); end
    def self.hexdigest(name, data); new(name).hexdigest(data); end

    def _tag
      TAGS[@name] or raise OpenSSL::OpenSSLError, "unsupported digest algorithm: #{@name}"
    end
    private :_tag

    # Streaming API: accumulate via update/<<, finalize via digest.
    def update(data); @buffer += data.to_s.b; self; end
    alias << update
    def reset; @buffer = "".b; self; end

    # `digest(data)` / `hexdigest(data)` are one-shot (don't disturb the
    # streamed buffer); the no-arg forms finalize the buffer.
    def digest(data = nil)
      RubyrsDigest.digest(_tag, data.nil? ? @buffer : data.to_s)
    end

    def hexdigest(data = nil)
      digest(data).unpack1("H*")
    end

    def digest_length
      LENS[@name] || digest.bytesize
    end
    alias size digest_length

    class SHA256 < Digest
      def initialize(*); super("SHA256"); end
      def self.digest(data); new.digest(data); end
      def self.hexdigest(data); new.hexdigest(data); end
    end
    class SHA1 < Digest
      def initialize(*); super("SHA1"); end
      def self.digest(data); new.digest(data); end
      def self.hexdigest(data); new.hexdigest(data); end
    end
    class SHA512 < Digest
      def initialize(*); super("SHA512"); end
      def self.digest(data); new.digest(data); end
      def self.hexdigest(data); new.hexdigest(data); end
    end
    class SHA384 < Digest
      def initialize(*); super("SHA384"); end
      def self.digest(data); new.digest(data); end
      def self.hexdigest(data); new.hexdigest(data); end
    end
    class MD5 < Digest
      def initialize(*); super("MD5"); end
      def self.digest(data); new.digest(data); end
      def self.hexdigest(data); new.hexdigest(data); end
    end
  end

  # `OpenSSL::HMAC.digest(digest, key, data)` — keyed-hash MAC (RFC 2104).
  # `digest` is either an `OpenSSL::Digest` instance or an algorithm-name
  # String. SHA-256 uses the verified native fast path; every other
  # algorithm `OpenSSL::Digest` supports (SHA1 / SHA512 / MD5) runs the
  # pure-Ruby HMAC construction over that digest.
  module HMAC
    def self.digest(digest, key, data)
      algo = (digest.respond_to?(:name) ? digest.name : digest.to_s).upcase.gsub("-", "")
      if algo == "SHA256"
        __rubyrs_hmac_sha256(key.to_s, data.to_s)
      else
        __generic(algo, key.to_s, data.to_s)
      end
    end

    def self.hexdigest(digest, key, data)
      digest(digest, key, data).unpack1("H*")
    end

    # RFC 2104 over any OpenSSL::Digest algorithm. The block size is
    # 128 bytes for the SHA-512 family, 64 for the rest.
    def self.__generic(algo, key, data)
      block = (algo == "SHA512" || algo == "SHA384") ? 128 : 64
      # `.b` keeps every intermediate ASCII-8BIT — the digest output is
      # UTF-8-tagged, which would clash when concatenated with the
      # BINARY pads (e.g. a key longer than the block, hashed down).
      hash = ->(d) { OpenSSL::Digest.new(algo).digest(d).b }
      k = key.b
      k = hash.call(k) if k.bytesize > block
      k += ("\x00".b * (block - k.bytesize)) if k.bytesize < block
      # `pack("C*")` (not map{.chr}.join) — Array#join re-encodes
      # ASCII-8BIT bytes >127 to UTF-8, corrupting the keystream.
      ipad = k.bytes.map { |b| b ^ 0x36 }.pack("C*")
      opad = k.bytes.map { |b| b ^ 0x5c }.pack("C*")
      hash.call(opad + hash.call(ipad + data.b))
    end
  end

  # `OpenSSL::KDF.pbkdf2_hmac` (RFC 2898) — derive a key from a password
  # via iterated HMAC. Built on OpenSSL::HMAC, so it works for any
  # supported digest. `OpenSSL::PKCS5.pbkdf2_hmac` is the legacy alias.
  module KDF
    def self.pbkdf2_hmac(pass, salt:, iterations:, length:, hash:)
      algo = hash.respond_to?(:name) ? hash.name : hash.to_s
      pass = pass.to_s
      salt = salt.to_s
      dk = "".b
      block_index = 1
      while dk.bytesize < length
        u = OpenSSL::HMAC.digest(algo, pass, salt + [block_index].pack("N"))
        t = u
        (iterations - 1).times do
          u = OpenSSL::HMAC.digest(algo, pass, u)
          t = t.bytes.zip(u.bytes).map { |a, b| a ^ b }.pack("C*")
        end
        dk += t
        block_index += 1
      end
      dk[0, length]
    end
  end

  module PKCS5
    def self.pbkdf2_hmac(pass, salt, iterations, length, digest)
      OpenSSL::KDF.pbkdf2_hmac(pass, salt: salt, iterations: iterations,
                               length: length, hash: digest)
    end

    # CRuby's older SHA-1-fixed entry point.
    def self.pbkdf2_hmac_sha1(pass, salt, iterations, length)
      OpenSSL::KDF.pbkdf2_hmac(pass, salt: salt, iterations: iterations,
                               length: length, hash: "SHA1")
    end
  end

  # Constant-time comparison. `fixed_length_secure_compare` requires
  # equal-length inputs (raises otherwise); `secure_compare` returns
  # false on a length mismatch first (matching CRuby).
  def self.fixed_length_secure_compare(a, b)
    a = a.to_s.b
    b = b.to_s.b
    raise ArgumentError, "inputs must be of equal length" if a.bytesize != b.bytesize
    res = 0
    a.bytes.zip(b.bytes).each { |x, y| res |= x ^ y }
    res.zero?
  end

  def self.secure_compare(a, b)
    return false unless a.to_s.bytesize == b.to_s.bytesize
    fixed_length_secure_compare(a, b)
  end

  # `OpenSSL::Cipher` — AES-256 in CTR or GCM mode (the native AES core
  # is 256-bit-only). CTR is a stream cipher: `update` streams
  # immediately (tracking the keystream byte offset so split updates
  # resume) and `final` is empty. GCM is authenticated: `update` buffers
  # and `final` runs the one-shot encrypt/decrypt — emitting the
  # ciphertext (and capturing #auth_tag) on encrypt, or verifying the
  # tag and returning the plaintext on decrypt (raising on mismatch).
  class Cipher
    class CipherError < OpenSSL::OpenSSLError; end

    def initialize(name)
      n = name.to_s.downcase
      m = n.match(/\Aaes-(128|192|256)-(ctr|gcm|cbc)\z/)
      unless m
        raise CipherError, "unsupported cipher #{name.inspect} " \
                           "(only aes-{128,192,256}-{ctr,gcm,cbc})"
      end
      @name = n
      @key_len = m[1].to_i / 8        # 16 / 24 / 32 bytes
      mode = m[2]
      @gcm = (mode == "gcm")
      @cbc = (mode == "cbc")
      # GCM and CBC buffer the whole message (GCM for the tag, CBC for
      # the block-aligned PKCS#7 pad); CTR streams directly.
      @buffered = @gcm || @cbc
      @key = nil
      @iv = nil
      @offset = 0
      @mode = :encrypt
      @aad = "".b
      @buffer = "".b
      @auth_tag = nil
      @padding = true
    end

    # Mode selectors — reset the stream position / GCM buffer.
    def encrypt; @mode = :encrypt; @offset = 0; @buffer = "".b; self; end
    def decrypt; @mode = :decrypt; @offset = 0; @buffer = "".b; self; end

    def key=(k)
      k = k.to_s
      # CRuby raises ArgumentError (not CipherError) on a wrong-size key.
      unless k.bytesize == @key_len
        raise ArgumentError, "key must be #{@key_len} bytes"
      end
      @key = k.dup.force_encoding("BINARY")
      @offset = 0
      k
    end

    def iv=(v)
      v = v.to_s
      # CTR needs a full 16-byte counter; GCM's IV is variable (12 is
      # standard and what Rails / MessageEncryptor use).
      unless @gcm || v.bytesize == 16
        raise ArgumentError, "iv must be 16 bytes"
      end
      @iv = v.dup.force_encoding("BINARY")
      @offset = 0
      v
    end

    def key_len; @key_len; end
    def iv_len; @gcm ? 12 : 16; end

    # PKCS#7 padding toggle (CBC). `padding = 0` requires block-aligned
    # input. CRuby accepts an integer; non-zero / true means on.
    def padding=(v); @padding = !(v == 0 || v == false); v; end

    # GCM authenticated-data + tag accessors (no-ops / errors for CTR).
    def auth_data=(a); @aad = a.to_s.b; a; end
    def auth_tag=(t); @auth_tag = t.to_s.b; t; end
    def auth_tag(len = 16)
      raise CipherError, "auth_tag not available" if @auth_tag.nil?
      @auth_tag[0, len]
    end

    def random_iv
      self.iv = SecureRandom.random_bytes(iv_len)
      @iv
    end

    def random_key
      self.key = SecureRandom.random_bytes(@key_len)
      @key
    end

    def update(data)
      raise CipherError, "cipher key not set" if @key.nil?
      raise CipherError, "cipher iv not set" if @iv.nil?
      data = data.to_s
      if @buffered
        # GCM / CBC need the whole message — buffer here, emit in final
        # (so `update(x) + final` yields the full result).
        @buffer += data.b
        "".b
      else
        out = __rubyrs_aes_ctr(@key, @iv, @offset, data)
        @offset += data.bytesize
        out
      end
    end

    def final
      return "".dup.force_encoding("BINARY") unless @buffered
      return gcm_final if @gcm
      cbc_final
    end

    private

    def gcm_final
      if @mode == :decrypt
        raise CipherError, "auth_tag not set" if @auth_tag.nil?
        pt = __rubyrs_aes_gcm_decrypt(@key, @iv, @aad, @buffer, @auth_tag)
        raise CipherError, "bad decrypt" if pt.nil?
        pt
      else
        res = __rubyrs_aes_gcm_encrypt(@key, @iv, @aad, @buffer)
        @auth_tag = res[-16..].b
        res[0...-16].b
      end
    end

    def cbc_final
      if @mode == :decrypt
        if @padding
          raise CipherError, "bad decrypt" unless (@buffer.bytesize % 16).zero? && !@buffer.empty?
        end
        pt = __rubyrs_aes_cbc_decrypt(@key, @iv, @buffer)
        return pt unless @padding
        pad = pt.getbyte(pt.bytesize - 1)
        # Valid PKCS#7: 1..16, and the last `pad` bytes all equal `pad`.
        ok = pad && pad >= 1 && pad <= 16 && pad <= pt.bytesize &&
             pt[(pt.bytesize - pad)..].bytes.all? { |b| b == pad }
        raise CipherError, "bad decrypt" unless ok
        pt[0, pt.bytesize - pad]
      else
        data = @buffer
        if @padding
          pad = 16 - (data.bytesize % 16)
          data += ([pad] * pad).pack("C*")
        elsif (data.bytesize % 16) != 0
          raise CipherError, "data not a multiple of the block length"
        end
        __rubyrs_aes_cbc_encrypt(@key, @iv, data)
      end
    end
  end

  module SSL
    class SSLError < OpenSSL::OpenSSLError; end

    VERIFY_NONE                 = 0
    VERIFY_PEER                 = 1
    VERIFY_FAIL_IF_NO_PEER_CERT = 2
    VERIFY_CLIENT_ONCE          = 4

    TLS1_VERSION   = 0x0301
    TLS1_1_VERSION = 0x0302
    TLS1_2_VERSION = 0x0303
    TLS1_3_VERSION = 0x0304

    # Net::HTTP builds one of these and sets verify_mode / versions on it.
    # v1 honours `verify_mode` (PEER vs NONE); other knobs are accepted
    # best-effort (webpki-roots is the only trust store — ADR 0029 §5).
    class SSLContext
      attr_accessor :verify_mode, :verify_hostname, :min_version, :max_version,
                    :options, :ca_file, :ca_path, :cert_store, :ciphers,
                    :cert, :key, :timeout

      def initialize(_version = nil)
        @verify_mode = OpenSSL::SSL::VERIFY_PEER
        @verify_hostname = true
      end

      # net/http guards its session-cache setup on
      # `unless @ssl_context.session_cache_mode.nil?` — returning nil
      # makes it skip the cache config (and the SESSION_CACHE_* consts we
      # don't ship). `session_new_cb` is intentionally absent so
      # net/http's `respond_to?(:session_new_cb)` guard also skips.
      def session_cache_mode; nil; end

      # Net::HTTP calls `ctx.set_params(params)` with a Hash of the above.
      def set_params(params = {})
        params.each do |k, v|
          setter = "#{k}="
          send(setter, v) if respond_to?(setter)
        end
        @verify_mode ||= OpenSSL::SSL::VERIFY_PEER
        self
      end
    end

    # Wraps a connected `TCPSocket`; `#connect` performs the TLS
    # handshake (taking the underlying stream into the native TLS
    # session) and the read/write surface then routes through TLS.
    class SSLSocket
      attr_accessor :hostname
      attr_reader :context, :io

      def initialize(io, context = nil)
        @io = io
        @context = context || SSLContext.new
        @hostname = nil
        @ssl = nil
        @closed = false
        @sync_close = false
      end

      def sync_close=(v); @sync_close = v; end
      def sync_close; @sync_close; end

      def connect
        return self if @ssl
        verify = (@context && @context.verify_mode == OpenSSL::SSL::VERIFY_NONE) ? 0 : 1
        # net/http skips `hostname=` when connecting to a literal IP
        # (SNI is invalid for IPs per RFC 6066), so @hostname can be nil
        # — fall back to the socket's peer host (an IP string parses as a
        # rustls IpAddress ServerName).
        sni = @hostname.to_s
        if sni.empty? && @io.respond_to?(:peeraddr)
          sni = @io.peeraddr[2].to_s
        end
        @ssl = __rubyrs_openssl_connect(@io.__rubyrs_handle, sni, verify)
        self
      end

      # net/protocol's `ssl_socket_connect` drives
      # `s.connect_nonblock(exception: false)` in a select loop. The
      # battery handshake is synchronous (blocking), so do the full
      # connect and return self — never :wait_readable — and the loop's
      # `else` branch breaks immediately.
      def connect_nonblock(*_args, **_kw)
        connect
      end

      def read_nonblock(maxlen, outbuf = nil, exception: true)
        data = __rubyrs_openssl_read(@ssl, maxlen.to_i)
        if data.nil? # EOF / close_notify
          return nil if exception == false
          raise EOFError, "end of file reached"
        end
        if outbuf
          outbuf.replace(data)
          return outbuf
        end
        data
      end

      def write_nonblock(str, exception: true)
        __rubyrs_openssl_write(@ssl, str.to_s)
      end

      def write(*strs)
        n = 0
        strs.each { |s| n += __rubyrs_openssl_write(@ssl, s.to_s) }
        n
      end

      def <<(str)
        __rubyrs_openssl_write(@ssl, str.to_s)
        self
      end

      def read(len = nil, outbuf = nil)
        if len
          result = __rubyrs_openssl_read(@ssl, len.to_i)
        else
          buf = +""
          buf.force_encoding(Encoding::BINARY) if buf.respond_to?(:force_encoding)
          while (chunk = __rubyrs_openssl_read(@ssl, 65_536))
            buf << chunk
          end
          result = buf
        end
        if outbuf && result
          outbuf.replace(result)
          return outbuf
        end
        result
      end

      def to_io; self; end
      def wait_readable(*); true; end
      def wait_writable(*); true; end
      def closed?; @closed; end

      def close
        return nil if @closed
        __rubyrs_openssl_close(@ssl) if @ssl
        @closed = true
        @io.close if @sync_close && @io.respond_to?(:close)
        nil
      end

      def flush; self; end
      def sync; true; end
      def sync=(v); v; end
      def setsockopt(*_args); self; end

      # Verification is enforced host-side during the handshake; once
      # `connect` returns the peer is already trusted (VERIFY_PEER) or
      # explicitly skipped (VERIFY_NONE), so the post-check is a no-op.
      def post_connection_check(_hostname); true; end
      # Peer-cert introspection isn't surfaced in v1 (ADR 0029 §7).
      def peer_cert; nil; end

      # net/http logs `s.ssl_version` / `s.cipher[0]` after the
      # handshake. v1 doesn't surface the negotiated suite from rustls,
      # so report representative values (cosmetic — debug output only).
      def ssl_version; "TLSv1.3"; end
      def cipher; ["TLS_AES_128_GCM_SHA256", "TLSv1.3", 128, 128]; end

      def peeraddr(*_rest)
        @io.respond_to?(:peeraddr) ? @io.peeraddr : ["AF_INET", 0, "", ""]
      end
    end
  end
end
