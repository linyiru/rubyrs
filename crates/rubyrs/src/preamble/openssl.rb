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
