# `_socket` battery — pure-Ruby `TCPSocket` veneer over the
# `__rubyrs_socket_*` host fns (ADR 0028). Single-layer discipline: the
# host fns are internal; THIS class is the user-facing, parity-tested
# surface (bare `require "socket"` / used by `net/http`). The veneer
# exposes the `read_nonblock` / `write_nonblock` contract net/protocol's
# BufferedIO drives, mapped onto the blocking host fns — `read_nonblock`
# returns bytes or nil(EOF) and NEVER `:wait_readable`, so the
# readiness-wait path is dead code (ADR 0028 §1).

# net/http evaluates `Socket::IPPROTO_TCP` / `Socket::TCP_NODELAY` as
# setsockopt args; TCP_NODELAY itself is folded into connect host-side
# (§1.3), so these constants just need to resolve.
module Socket
  IPPROTO_TCP = 6
  TCP_NODELAY = 1
  SOL_SOCKET  = 1
  SO_KEEPALIVE = 8
  AF_INET     = 2
  SOCK_STREAM = 1
end

# Raised on DNS / connect / reset failures the host-fn layer can't map to
# a specific Errno (matches MRI's SocketError < StandardError).
class SocketError < StandardError; end

class TCPSocket
  def self.open(host, port, *_rest)
    new(host, port)
  end

  def initialize(host, port, *_rest)
    @host = host.to_s
    @port = port.to_i
    @read_timeout = nil
    @handle = __rubyrs_socket_connect(@host, @port)
    @closed = false
  end

  # net/protocol BufferedIO read path:
  #   @io.read_nonblock(BUFSIZE, tmp, exception: false)
  # The host fn blocks (bounded by @read_timeout) and returns bytes or
  # nil(EOF) — never :wait_readable.
  def read_nonblock(maxlen, outbuf = nil, exception: true)
    data = __rubyrs_socket_read(@handle, maxlen.to_i, @read_timeout)
    if data.nil? # EOF
      return nil if exception == false
      raise EOFError, "end of file reached"
    end
    if outbuf
      outbuf.replace(data)
      return outbuf
    end
    data
  end

  # net/protocol BufferedIO write path:
  #   @io.write_nonblock(str, exception: false)
  def write_nonblock(str, exception: true)
    __rubyrs_socket_write(@handle, str.to_s)
  end

  def write(*strs)
    n = 0
    strs.each { |s| n += __rubyrs_socket_write(@handle, s.to_s) }
    n
  end

  def <<(str)
    __rubyrs_socket_write(@handle, str.to_s)
    self
  end

  # Plain blocking `read` (full-drain when len is nil, else up to len).
  # net/http mostly reads through BufferedIO#read_nonblock; this covers
  # direct `socket.read` callers.
  def read(len = nil, outbuf = nil)
    if len
      chunk = __rubyrs_socket_read(@handle, len.to_i, @read_timeout)
      result = chunk.nil? ? nil : chunk
    else
      buf = +""
      buf.force_encoding(Encoding::BINARY) if buf.respond_to?(:force_encoding)
      while (chunk = __rubyrs_socket_read(@handle, 65_536, @read_timeout))
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

  # `to_io` returns self; `wait_readable`/`wait_writable` are only reached
  # if read_nonblock ever returns :wait_readable (it doesn't, blocking
  # battery), so they're conservative no-ops.
  def to_io; self; end
  def wait_readable(*); true; end
  def wait_writable(*); true; end

  def closed?; @closed; end

  def close
    return nil if @closed
    __rubyrs_socket_close(@handle)
    @closed = true
    nil
  end

  # TCP_NODELAY is set inside the connect host fn (ADR 0028 §1.3); any
  # other sockopt is accepted and ignored.
  def setsockopt(*_args); self; end
  def getsockopt(*_args); nil; end

  def read_timeout=(t); @read_timeout = t; end
  def read_timeout; @read_timeout; end

  def flush; self; end
  def sync; true; end
  def sync=(v); v; end
  def eof?; false; end

  def peeraddr(*_rest)
    ["AF_INET", @port, @host, @host]
  end
end
