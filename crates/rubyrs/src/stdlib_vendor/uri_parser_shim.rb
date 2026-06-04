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

    def unescape(str)
      # Byte-level scan + `Array#pack('C*')` at the end. We
      # deliberately do NOT use `gsub(/%XX/) { ... }`: when the
      # block returns a binary single-byte string (e.g. 0xE4 from
      # decoding `%E4`), rubyrs's current gsub-string-reassembly
      # path views the result as UTF-8 and rewrites every invalid
      # byte to U+FFFD (3 bytes), corrupting multi-byte sequences
      # like `%E4%B8%AD` (中) into `���`. Collecting the decoded
      # bytes into an Int Array and packing once sidesteps that
      # — `Array#pack('C*')` writes the bytes raw and the
      # resulting String carries them through unchanged. CRuby
      # also returns a binary-encoded String here; Rack's
      # `Utils.unescape` calls `.force_encoding(encoding)` after.
      src = str.to_s.bytes
      n = src.length
      out = []
      i = 0
      hex_val = lambda do |b|
        case b
        when 48..57  then b - 48   # '0'..'9'
        when 65..70  then b - 55   # 'A'..'F'
        when 97..102 then b - 87   # 'a'..'f'
        end
      end
      while i < n
        if src[i] == 37 && i + 2 < n \
            && (h = hex_val.call(src[i + 1])) \
            && (l = hex_val.call(src[i + 2]))
          out << (h * 16 + l)
          i += 3
        else
          out << src[i]
          i += 1
        end
      end
      out.pack('C*')
    end
  end

  DEFAULT_PARSER = RFC2396_Parser.new
  # Sinatra 4 / Rack 3 prefer `RFC2396_PARSER` (capitalized
  # constant) when present; alias to the same instance so the
  # `defined?` probe at top of rack/utils.rb takes the
  # `RFC2396_PARSER` branch and `URI_PARSER` ends up pointing at
  # one object (lets identity-checks line up too).
  RFC2396_PARSER = DEFAULT_PARSER
end
