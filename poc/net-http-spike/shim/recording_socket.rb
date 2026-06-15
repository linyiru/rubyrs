# Recording TCPSocket shim for the net/http discovery spike (ADR 0028
# Phase 1). Stands in for the future `_socket` native battery: it logs
# EVERY method net/protocol's BufferedIO + net/http drive on the socket,
# and feeds back a canned HTTP/1.1 response so a real request completes.
# The point is to enumerate the exact host-fn surface Phase 3 must
# implement — not to be a real socket.

$NH_CALLS = Hash.new(0)   # method name => call count (the host-fn surface)

# NOTE: `String#chop`, `String#clear`, and `Errno::EALREADY` /
# `ECONNABORTED` were the spike's discovered Tier-1 gaps — now LANDED in
# rubyrs core (feat(string)/feat(errno)), so no longer shimmed here.

def nh_log(m)
  $NH_CALLS[m] += 1
end

# Canned response the recording socket plays back to net/http's reader.
CANNED_RESPONSE = (+"HTTP/1.1 200 OK\r\n") <<
  "Content-Type: text/plain\r\n" <<
  "Content-Length: 13\r\n" <<
  "Connection: close\r\n" <<
  "\r\n" <<
  "hello, world!"

# Stub the load-time requires net/protocol + net/http pull that rubyrs
# doesn't vendor, so the real net/http.rb can load and run.
module Kernel
  alias_method :__nh_orig_require, :require unless private_method_defined?(:__nh_orig_require) || method_defined?(:__nh_orig_require)
  def require(name)
    case name
    when "socket", "io/wait", "resolv", "openssl", "net/https", "pp"
      return true   # surface provided by the shim below / not exercised
    end
    __nh_orig_require(name)
  end
end

# NOTE: `uri` is no longer shimmed — the REAL `uri` gem now loads and
# parses on rubyrs (after the const_defined?(name,false) + String#delete!
# fixes). The probe puts uri-1.0.2 on $LOAD_PATH; net/http uses it directly.

# A minimal IO-ish object net/protocol's `@io.to_io.wait_readable` path
# expects. The recording socket returns one of these from `to_io`.
class RecordingWaitable
  def wait_readable(*) ; nh_log("to_io.wait_readable"); true; end
  def wait_writable(*) ; nh_log("to_io.wait_writable"); true; end
end

# The recording socket. TCPSocket.open/new returns one of these.
class RecordingSocket
  def initialize(host, port)
    nh_log("connect(host,port)")
    @host = host
    @port = port
    @read_buf = CANNED_RESPONSE.dup
    @written = +""
    @closed = false
    @waitable = RecordingWaitable.new
  end

  # --- the net/protocol BufferedIO read path ---
  def read_nonblock(maxlen, buf = nil, exception: true)
    nh_log("read_nonblock")
    if @read_buf.empty?
      return nil if exception == false   # net/protocol treats nil as EOF
      raise EOFError
    end
    chunk = @read_buf.slice!(0, maxlen)
    if buf
      buf.replace(chunk)
      return buf
    end
    chunk
  end

  # --- the net/protocol BufferedIO write path ---
  def write_nonblock(str, exception: true)
    nh_log("write_nonblock")
    @written << str.to_s
    str.to_s.bytesize
  end

  # net/http sometimes writes via plain write (header/body split paths)
  def write(*strs)
    nh_log("write")
    n = 0
    strs.each { |s| @written << s.to_s; n += s.to_s.bytesize }
    n
  end

  def <<(s) ; nh_log("<<"); @written << s.to_s; self; end

  def to_io      ; nh_log("to_io"); @waitable; end
  def eof?       ; nh_log("eof?"); @read_buf.empty?; end
  def closed?    ; nh_log("closed?"); @closed; end
  def close      ; nh_log("close"); @closed = true; nil; end
  def flush      ; nh_log("flush"); self; end
  def sync       ; nh_log("sync"); true; end
  def sync=(v)   ; nh_log("sync="); v; end
  def setsockopt(*) ; nh_log("setsockopt"); 0; end
  def peeraddr(*)   ; nh_log("peeraddr"); ["AF_INET", @port, @host, "127.0.0.1"]; end
  def local_address ; nh_log("local_address"); nil; end
  def remote_address; nh_log("remote_address"); nil; end

  # Catch-all so we DISCOVER any method we didn't anticipate rather than
  # crash — the unknown call is logged with a `?` marker for FINDINGS.
  def method_missing(m, *args, &blk)
    nh_log("UNHANDLED:#{m}")
    nil
  end
  def respond_to_missing?(m, include_all = false) ; true; end
end

# TCPSocket factory net/http uses.
class TCPSocket
  def self.open(host, port, *rest)
    nh_log("TCPSocket.open")
    RecordingSocket.new(host, port)
  end
  def self.new(host, port, *rest)
    nh_log("TCPSocket.new")
    RecordingSocket.new(host, port)
  end
end

class SocketError < StandardError; end

# net/http does `s.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)`.
# The real `_socket` battery will expose these (or net/http.rb will be
# trimmed to drop the TCP_NODELAY tweak). Spike provides the constants.
module Socket
  IPPROTO_TCP = 6
  TCP_NODELAY = 1
  AF_INET     = 2
  SOCK_STREAM = 1
end

# (The Errno classes faraday-net_http references — EALREADY, ECONNABORTED,
# … — now all exist in rubyrs core; the spike no longer stubs them.)

# faraday's logging formatter requires "pp" for Object#pretty_inspect.
module Kernel
  def pretty_inspect ; "#{inspect}\n"; end
end
