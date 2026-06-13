# URI::DEFAULT_PARSER / URI::RFC2396_PARSER — the minimum surface
# rack/utils.rb needs at module-load time.
#
# Rack 3 has this at the top of `lib/rack/utils.rb`:
#
#   URI_PARSER = defined?(::URI::RFC2396_PARSER) ?
#                  ::URI::RFC2396_PARSER : ::URI::DEFAULT_PARSER
#
# That assignment fires while requiring `rack/utils` — i.e. before
# any request handling — so unless at least one of the two
# constants is defined, the require itself raises
# `NameError: uninitialized constant URI::DEFAULT_PARSER` and
# blocks every Sinatra / Rack-based app from even loading.
#
# This shim is loaded unconditionally by the `require "uri"`
# lenient-stub path (kernel.rs require dispatch), not gated behind
# the `stdlib` feature, because Sinatra-on-rubyrs in the default
# build relies on it. The implementation is just enough for the
# two methods Rack actually calls on `URI_PARSER`:
#
#   - URI_PARSER.escape(s [, unsafe_regexp])  — percent-encode
#   - URI_PARSER.unescape(s)                  — percent-decode
#
# The full RFC 2396 / 3986 parser surface (`parse`, `extract`,
# `make_regexp`, etc.) remains absent in the default build — see
# `--features stdlib` for a fuller URI implementation when one is
# vendored. Callers that hit those methods get NoMethodError, which
# is the right "feature absent" signal (ADR 0017).

# Idempotency guard. `loaded_stdlib_stubs` (kernel.rs) dedups
# per raw require path, so `require "uri"` followed by
# `require "uri/common"` (or vice versa) lands in the lenient
# stub branch twice — once per distinct path key. Without this
# guard the shim would re-evaluate and the `DEFAULT_PARSER =
# RFC2396_Parser.new` line would replace the existing instance,
# silently breaking any Ruby code that has already memoized a
# reference to the parser (e.g. `URI_PARSER = ::URI::RFC2396_PARSER`
# at the top of rack/utils.rb).
#
# `defined?(URI::DEFAULT_PARSER)` returns `"constant"` on a
# second load and `nil` on the first; the `unless` skips the
# class+constant rebuild entirely on subsequent loads, preserving
# instance identity across all three subpath aliases.
unless defined?(URI::DEFAULT_PARSER)

module URI
  class RFC2396_Parser
    # Default "unsafe" character set used when no second arg is
    # passed. Matches CRuby's RFC 2396 parser default for
    # `escape(s)` (every char NOT in the unreserved + reserved +
    # mark sets gets percent-encoded). Rack normally passes its
    # own pattern (`Rack::Utils::PATH_UNSAFE`) so this is mainly
    # for compatibility with bare `escape(s)` callers.
    DEFAULT_UNSAFE_REGEXP = /[^a-zA-Z0-9\-_.!~*'();\/?:@&=+$,]/

    def escape(str, unsafe = DEFAULT_UNSAFE_REGEXP)
      str.to_s.gsub(unsafe) do |m|
        # `m` is a single matched character; for multi-byte UTF-8
        # it can be more than one byte. Percent-encode every byte
        # as `%XX` (uppercase hex, CRuby parity). `String#bytes`
        # returns an Array of byte ints — preferred over
        # `each_byte` because rubyrs's default subset doesn't
        # expose the enumerator-returning form.
        m.bytes.map { |b| "%%%02X" % b }.join
      end
    end

    # `make_regexp(schemes = nil)` — CRuby returns its precompiled
    # `@regexp[:ABS_URI_REF]` (an absolute URI reference matcher)
    # when no schemes are given. Rack 3 `rack/lint.rb:15` writes
    #   REQUEST_PATH_ABSOLUTE_FORM = /\A#{Utils::URI_PARSER.make_regexp}\z/
    # at module-load time, interpolating the result. Just needs to
    # return SOME Regexp whose `.to_s` (`(?-mix:...)` shape) can be
    # spliced. Actual REQUEST_PATH_ABSOLUTE_FORM lookups happen at
    # request-validation time which the spike never reaches.
    #
    # The pattern below is intentionally minimal — a permissive
    # absolute-URI shape (`scheme:rest`); the spike's URL handling
    # never matches against this Regexp during gem load.
    ABS_URI_REF_REGEXP = /[a-zA-Z][a-zA-Z0-9+.\-]*:\S+/
    def make_regexp(schemes = nil)
      ABS_URI_REF_REGEXP
    end

    def unescape(str)
      # CRuby's canonical shape: scan for percent-escapes, decode
      # each two-hex-digit capture back to its raw byte. Block
      # returns a binary single-byte String built via
      # `Array#pack('C')`; gsub splices those bytes verbatim into
      # the result, preserving multi-byte UTF-8 sequences across
      # the encode/decode roundtrip.
      #
      # Result encoding matches CRuby: bytes assembled via
      # `pack('C')` are ASCII-8BIT (binary) — the resulting
      # String carries those raw bytes through without imposing a
      # text encoding interpretation. `Rack::Utils.unescape`
      # always calls `.force_encoding(encoding)` after this
      # (default UTF-8) to retag the binary result back to the
      # caller's preferred encoding, so neither side ever assumes
      # what `unescape` returns is already valid UTF-8.
      str.to_s.gsub(/%([0-9A-Fa-f]{2})/) do
        [$1.to_i(16)].pack('C')
      end
    end
  end

  DEFAULT_PARSER = RFC2396_Parser.new
  # Sinatra 4 / Rack 3 prefer `RFC2396_PARSER` (capitalized
  # constant) when present; alias to the same instance so the
  # `defined?` probe at top of rack/utils.rb takes the
  # `RFC2396_PARSER` branch and `URI_PARSER` ends up pointing at
  # one object (lets identity-checks line up too).
  RFC2396_PARSER = DEFAULT_PARSER
  # CRuby's `URI::Parser` is an alias for its current parser class
  # (RFC3986 in 3.x). Rack's MockRequest memoizes
  # `URI::Parser.new` for parse_uri_rfc2396; point the alias at
  # the shim class so that instantiation works. (`parse` is still
  # absent — callers get NoMethodError, the right feature-absent
  # signal, until a fuller URI lands.)
  Parser = RFC2396_Parser

  # The www-form component pair (CRuby lib/uri/common.rb). These
  # back `Rack::Utils.escape` / `.unescape` and the query parsers.
  #
  # Both are implemented BYTE-level on purpose: percent-encoding
  # is defined over bytes, and `gsub`-over-chars would push
  # registry-tagged receivers (`"ø".encode("ISO-8859-1")`) through
  # the lossy char view — the settled Regexp-over-non-UTF-8
  # boundary (docs/SUBSET.md). Byte loops sidestep that entirely:
  # escape("ø" as ISO-8859-1) is "%F8", matching CRuby.
  def self.encode_www_form_component(str, enc = nil)
    str = str.to_s
    # CRuby transcodes only when an explicit target encoding is
    # given (with :replace fallbacks we don't mirror — documented
    # simplification; rack never passes `enc`).
    str = str.encode(enc) if enc && str.encoding != enc
    out = +""
    str.bytes.each do |b|
      if (b >= 0x30 && b <= 0x39) || (b >= 0x41 && b <= 0x5A) ||
         (b >= 0x61 && b <= 0x7A) ||
         b == 0x2A || b == 0x2D || b == 0x2E || b == 0x5F
        # unreserved set: ALNUM *-._  (note: `~` is NOT in the
        # www-form set — CRuby encodes it as %7E)
        out << b.chr
      elsif b == 0x20
        out << "+"
      else
        out << ("%%%02X" % b)
      end
    end
    out.force_encoding(Encoding::US_ASCII)
  end

  def self.decode_www_form_component(str, enc = Encoding::UTF_8)
    str = str.to_s
    bytes = str.bytes
    out = []
    i = 0
    n = bytes.length
    hex = lambda do |c|
      if c.nil? then nil
      elsif c >= 0x30 && c <= 0x39 then c - 0x30
      elsif c >= 0x41 && c <= 0x46 then c - 0x41 + 10
      elsif c >= 0x61 && c <= 0x66 then c - 0x61 + 10
      end
    end
    while i < n
      b = bytes[i]
      if b == 0x25 # %
        d1 = hex.call(bytes[i + 1])
        d2 = hex.call(bytes[i + 2])
        # CRuby validates the whole string against
        # %-followed-by-two-hex before decoding; the in-loop check
        # is equivalent (and O(n) — the spec pins the
        # catastrophic-backtracking regression from CRuby's old
        # regex validator, ruby-lang #5149).
        raise ArgumentError, "invalid %-encoding (#{str})" if d1.nil? || d2.nil?
        out << (d1 * 16 + d2)
        i += 3
      elsif b == 0x2B # +
        out << 0x20
        i += 1
      else
        out << b
        i += 1
      end
    end
    out.pack("C*").force_encoding(enc)
  end

  class Error < StandardError; end
  class InvalidURIError < Error; end

  # Minimal URI value object — the surface
  # `Rack::MockRequest.env_for` actually reads after
  # `URI::Parser#parse` (scheme / host / port / path= / query),
  # plus `to_s` for round-tripping. `port` is filled with the
  # scheme default when the authority omits it, matching CRuby's
  # post-parse view (`URI.parse("http://x/").port` → 80).
  class Generic
    attr_accessor :scheme, :userinfo, :host, :port, :path, :query, :fragment

    DEFAULT_PORTS = {
      "http" => 80, "https" => 443, "ftp" => 21,
      "ws" => 80, "wss" => 443,
    }.freeze

    def initialize(scheme, userinfo, host, port, path, query, fragment)
      @scheme = scheme
      @userinfo = userinfo
      @host = host
      @port = port || (scheme && DEFAULT_PORTS[scheme.downcase])
      @path = path
      @query = query
      @fragment = fragment
    end

    def to_s
      out = +""
      out << "#{@scheme}:" if @scheme
      if @host
        out << "//"
        out << "#{@userinfo}@" if @userinfo
        out << @host
        default = @scheme && DEFAULT_PORTS[@scheme.downcase]
        out << ":#{@port}" if @port && @port != default
      end
      out << @path.to_s
      out << "?#{@query}" if @query
      out << "##{@fragment}" if @fragment
      out
    end
  end

  class RFC2396_Parser
    # RFC 3986 appendix-B reference split (scheme / authority /
    # path / query / fragment), NOT a validating parser — it
    # accepts whatever it sees, which covers the
    # MockRequest.env_for shapes ("", "/", "/path?q=1",
    # "https://example.org:8080/x"). IPv6 bracket hosts are out
    # of subset (the rindex(":") port split would mis-cut them;
    # documented gap, no rack-spec consumer).
    SPLIT_RE = %r{\A(?:([A-Za-z][A-Za-z0-9+.\-]*):)?(?://([^/?\#]*))?([^?\#]*)(?:\?([^\#]*))?(?:\#(.*))?\z}m

    def parse(uri)
      m = SPLIT_RE.match(uri.to_s)
      raise InvalidURIError, "bad URI (is not URI?): #{uri.inspect}" unless m
      scheme = m[1]
      authority = m[2]
      userinfo = nil
      host = nil
      port = nil
      if authority
        rest = authority
        if at = rest.rindex("@")
          userinfo = rest[0, at]
          rest = rest[at + 1, rest.length]
        end
        if colon = rest.rindex(":")
          maybe_port = rest[colon + 1, rest.length]
          if maybe_port =~ /\A\d+\z/
            port = maybe_port.to_i
            rest = rest[0, colon]
          end
        end
        host = rest unless rest.empty?
      end
      # path is "" (never nil) when absent — env_for does
      # `uri.path[0]` and `(uri.path).b` unconditionally.
      Generic.new(scheme, userinfo, host, port, m[3] || "", m[4], m[5])
    end
  end

  def self.parse(uri)
    DEFAULT_PARSER.parse(uri)
  end
end

end # `unless defined?(URI::DEFAULT_PARSER)`
