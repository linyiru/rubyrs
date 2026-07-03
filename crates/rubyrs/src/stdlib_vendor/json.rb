# Tier 3 pure-Ruby JSON — subset matched to CRuby stdlib's
# `json` for the deterministic core (parse + generate over
# Null / Bool / Integer / Float / String / Array / Hash).
#
# Gated behind the `stdlib` Cargo feature (ADR 0017 row 125;
# ADR 0019 Rule 6 "pure-Ruby canon"; ADR 0026 v2 menu item 2).
# Default Tier-1 builds keep the lenient `require "json"` stub —
# the JSON constant exists but its methods raise NoMethodError.
# `--features stdlib` evaluates this file's body on the running
# Vm and gives `JSON.parse` / `JSON.generate` their real semantics.
#
# What this implements:
#   - `JSON.parse(str)` — recursive-descent parser; returns
#     Ruby values per the canonical mapping (null → nil, true →
#     true, false → false, integer → Integer, fraction/exponent
#     → Float, "..." → String, [...] → Array, {...} → Hash with
#     String keys).
#   - `JSON.generate(obj)` — serializer; emits compact form
#     (no whitespace) matching CRuby's `JSON.generate` default.
#     Symbol keys are stringified (matches CRuby's `to_s`-on-key
#     convention).
#   - `JSON::ParserError < StandardError` — surface raised on
#     malformed input; class name matches CRuby's so user
#     `rescue JSON::ParserError` clauses port.
#   - `JSON::GeneratorError < StandardError` — surface raised
#     on un-serializable values (NaN/Infinity Floats are rejected
#     by default per RFC 8259; cyclic refs are not detected —
#     trust the caller).
#
# What this DOES NOT implement (deferred — file follow-ups when
# concrete fixtures need them):
#   - Options: `allow_nan`, `quirks_mode`, `object_class`
#   - `JSON::Ext::Parser` / `JSON::Ext::Generator` (the C-ext
#     classes — flori-json-cext already covers that surface in
#     the `examples/` directory)
#
# Float divergence (ADR 0019 class `h`): both runtimes use
# Ruby's `Float#to_s`, so round-trip behaviour matches CRuby
# wherever `Float#to_s` does. Inputs at the edge of IEEE-754
# precision may diverge in the last digit between
# implementations; that's an accepted Rule 6 deviation.

module JSON
  # Exception hierarchy matches CRuby's `json` gem so user
  # code's `rescue JSON::JSONError` / `rescue JSON::ParserError`
  # / `rescue JSON::NestingError` clauses port unchanged.
  class JSONError < StandardError; end
  class ParserError < JSONError; end
  class GeneratorError < JSONError; end
  # CRuby: NestingError < ParserError (not a direct JSONError
  # child) — `rescue JSON::ParserError` must catch nesting
  # overflows too.
  class NestingError < ParserError; end

  # Default depth limit for parse + generate. Matches CRuby's
  # `JSON.generate`/`JSON.parse` default (100). Pass `false` or
  # `0` via `max_nesting:` to disable.
  MAX_NESTING_DEFAULT = 100

  # JSON::State — formatting + safety options bag. CRuby exposes
  # this as `JSON::Ext::Generator::State` (the C-ext flavour),
  # aliased here as the documented `JSON::State` constant.
  # User code that constructs / inspects a State instance to
  # configure `generate(obj, state)` ports unchanged.
  #
  # Init accepts a Hash (positional) so a trailing
  # `JSON::State.new(indent: "  ", max_nesting: 5)` caller
  # works the same as `JSON::State.new({indent: "  ", max_nesting: 5})`
  # — rubyrs + CRuby both coerce the trailing key/value pairs
  # into the positional Hash slot.
  class State
    attr_reader :indent, :space, :space_before, :object_nl, :array_nl, :max_nesting

    def initialize(opts = nil)
      opts = {} if opts.nil?
      @indent       = opts[:indent]       || ""
      @space        = opts[:space]        || ""
      @space_before = opts[:space_before] || ""
      @object_nl    = opts[:object_nl]    || ""
      @array_nl     = opts[:array_nl]     || ""
      @allow_nan    = opts[:allow_nan]    ? true : false
      # CRuby's State treats `max_nesting: 0` as "unlimited".
      # `nil` falls back to the default; `false` is the
      # documented opt-out marker (we encode it as 0 too).
      if opts.has_key?(:max_nesting)
        v = opts[:max_nesting]
        @max_nesting = (v == false || v.nil?) ? 0 : v
      else
        @max_nesting = MAX_NESTING_DEFAULT
      end
    end

    def allow_nan?
      @allow_nan
    end

    # Predicate accessor matching CRuby's; lets fixture / user
    # code `state.indent? ? ... : ...` if they need to detect a
    # formatting State without inspecting bytes.
    def indent?
      !@indent.empty?
    end
  end

  # ---- Parse ----

  # Detected once at load: are the `_json_native` host fns
  # registered? If yes, hot calls route through serde_json — same
  # Ruby Value shape, same emitted bytes, ~order-of-magnitude
  # faster on big payloads. If no, the pure-Ruby `Parser` /
  # `generate_with` recursion is the authoritative path.
  # `RUBYRS_JSON_NO_NATIVE=1` is the kill switch (debugging /
  # three-way parity testing — mirrors RUBYRS_PRISM_NO_NATIVE);
  # it disables the serde accelerator only, NOT the always-on
  # `__rubyrs_json_float_repr` float formatter (that one is part
  # of the canon's own emit contract).
  NATIVE_AVAILABLE = !ENV["RUBYRS_JSON_NO_NATIVE"] &&
    defined?(__rubyrs_json_native_parse) && defined?(__rubyrs_json_native_generate) ? true : false

  def self.parse(str, opts = nil)
    # Fast path: the overwhelmingly common `JSON.parse(str)` call
    # (no opts) skips all option digestion. The native host fn
    # itself validates the input shape; non-String input falls
    # through to the slow path's is_a? raise below.
    if opts.nil? && NATIVE_AVAILABLE && str.is_a?(String)
      begin
        return __rubyrs_json_native_parse(str)
      rescue RuntimeError
        # ANY native decline or serde-side parse error falls
        # through to the pure canon, which is the single authority
        # for both values (exact bigints, 1e999 overflow literals,
        # non-UTF-8 input, lone low surrogates) and error classes
        # + messages (strict number grammar, nesting rule, control
        # characters in strings). A second parse is paid only on
        # decline/error documents; the canon's own recursion is
        # bounded by its nesting guard.
      end
    end
    raise ParserError, "input must be a String" unless str.is_a?(String)
    symbolize = opts && opts[:symbolize_names] ? true : false
    max_nest = opts && opts.has_key?(:max_nesting) ? opts[:max_nesting] : MAX_NESTING_DEFAULT
    max_nest = 0 if max_nest == false || max_nest.nil?
    allow_nan = opts && opts[:allow_nan] ? true : false

    # Native fast path: serde_json handles the heavy lifting,
    # then if the caller asked for `symbolize_names: true` we
    # post-walk to convert String keys to Symbol. `allow_nan`
    # + `max_nesting` options stay on the pure path for now —
    # serde_json doesn't model "configurable nesting depth"
    # natively, and the canon's `max_nest > 0` guard would have
    # to be re-implemented on the Rust side. For the deterministic
    # default (allow_nan=false, max_nesting=100) AND the inputs
    # that don't approach the depth limit, the native path is
    # safe. Any caller that explicitly opted into deep nesting
    # or NaN tokens falls through to the canon below.
    if NATIVE_AVAILABLE && !allow_nan && max_nest == MAX_NESTING_DEFAULT
      begin
        v = __rubyrs_json_native_parse(str)
        return symbolize ? deep_symbolize_keys(v) : v
      rescue RuntimeError
        # Fall through to the pure canon — see the no-opts fast
        # path above: the canon is the single value + error
        # authority on any native decline or serde parse error.
      end
    end

    p = Parser.new(str, symbolize, max_nest, allow_nan)
    p.parse_top
  end

  # Walk a parsed tree converting Hash String-keys to Symbol.
  # Pure Ruby — runs only when the native fast path was taken
  # AND the caller passed `symbolize_names: true`. Arrays and
  # Hash values are traversed; non-collection values pass through.
  def self.deep_symbolize_keys(v)
    case v
    when Hash
      out = {}
      v.each { |k, val| out[k.is_a?(String) ? k.to_sym : k] = deep_symbolize_keys(val) }
      out
    when Array
      v.map { |x| deep_symbolize_keys(x) }
    else
      v
    end
  end

  # `JSON.parse!` — permissive parse with no nesting limit and
  # NaN/Infinity tokens accepted. Matches CRuby's `parse!`
  # default of `{max_nesting: false, allow_nan: true}`. User-
  # supplied opts override.
  def self.parse!(str, opts = nil)
    merged = { max_nesting: false, allow_nan: true }
    if opts
      opts.each { |k, v| merged[k] = v }
    end
    parse(str, merged)
  end

  # CRuby's `JSON.load` differs from `JSON.parse` in that it
  # historically accepted any IO-like input and permits non-
  # container roots in older Ruby; for the embedded-host
  # subset we treat it as a String-accepting alias of parse.
  # Callers that pass IO objects get the same NoMethodError as
  # any other unsupported input shape.
  def self.load(str)
    parse(str)
  end

  # `JSON[…]` shortcut — dispatches on input type: String input
  # parses; anything else generates. Mirrors CRuby's
  # `JSON::[]` convenience method (`JSON[json_str]` ↔
  # `JSON.parse(json_str)`; `JSON[obj]` ↔ `JSON.generate(obj)`).
  def self.[](v)
    v.is_a?(String) ? parse(v) : generate(v)
  end

  # Deprecated-but-still-present CRuby aliases. Some legacy code
  # bases still spell `JSON.unparse(obj)`; keeping them as
  # synonyms means rubyrs accepts the same surface without
  # forcing a rewrite.
  def self.unparse(obj, opts = nil)
    generate(obj, opts)
  end
  def self.pretty_unparse(obj, opts = nil)
    pretty_generate(obj, opts)
  end

  class Parser
    # fstring-equivalent key intern cache — PERSISTENT across
    # Parser instances (module-level), mirroring both CRuby's
    # process-global fstring table and the native visitor's
    # thread-local cache: object keys come out FROZEN and equal
    # key texts share one String object (`.equal?`) within AND
    # across separate JSON.parse calls, on every path. Capped
    # like the native cache; past the cap keys are still frozen,
    # just not shared.
    KEY_CACHE = {}
    KEY_CACHE_CAP = 8192

    def initialize(str, symbolize_names = false, max_nesting = MAX_NESTING_DEFAULT, allow_nan = false)
      @chars = str.chars
      @len = @chars.length
      @pos = 0
      @symbolize_names = symbolize_names
      @max_nesting = max_nesting
      @allow_nan = allow_nan
      @depth = 0
    end

    def parse_top
      skip_ws
      val = parse_value
      skip_ws
      if @pos < @len
        raise ParserError, "trailing data at position #{@pos}"
      end
      val
    end

    def peek
      return nil if @pos >= @len
      @chars[@pos]
    end

    def skip_ws
      while @pos < @len
        c = @chars[@pos]
        break unless c == " " || c == "\t" || c == "\n" || c == "\r"
        @pos += 1
      end
    end

    def parse_value
      c = peek
      raise ParserError, "unexpected end of input" if c.nil?
      case c
      when "{" then parse_object
      when "[" then parse_array
      when "\"" then parse_string
      when "t", "f" then parse_bool
      when "n"
        # `null` vs `NaN` discriminator — both start with `n`/`N`
        # but JSON only spells null lowercase. Capital `N` enters
        # the NaN path (allow_nan parser flag); the parse_null
        # arm errors out on anything that isn't exactly `null`.
        parse_null
      when "N" then parse_nan
      when "I", "-"
        # `Infinity` and `-Infinity` tokens (non-standard JSON,
        # accepted by parse! and JSON.parse(..., allow_nan: true)).
        # `-` ambiguous with negative number; the parse_number arm
        # handles the numeric branch + delegates here when the
        # next char is `I`.
        if c == "I" || (c == "-" && @pos + 1 < @len && @chars[@pos + 1] == "I")
          parse_infinity
        else
          parse_number
        end
      else
        if "0123456789".include?(c)
          parse_number
        else
          raise ParserError, "unexpected character '#{c}' at position #{@pos}"
        end
      end
    end

    def parse_nan
      if @allow_nan && @pos + 3 <= @len &&
        @chars[@pos] == "N" && @chars[@pos + 1] == "a" && @chars[@pos + 2] == "N"
        @pos += 3
        return 0.0 / 0.0
      end
      raise ParserError, "bad token at position #{@pos}"
    end

    def parse_infinity
      neg = false
      if @chars[@pos] == "-"
        neg = true
        @pos += 1
      end
      ident = "Infinity"
      if !@allow_nan || @pos + ident.length > @len
        raise ParserError, "bad token at position #{@pos}"
      end
      i = 0
      while i < ident.length
        if @chars[@pos + i] != ident[i]
          raise ParserError, "bad token at position #{@pos}"
        end
        i += 1
      end
      @pos += ident.length
      neg ? -1.0 / 0.0 : 1.0 / 0.0
    end

    # CRuby (json 2.20, probed): the nesting check fires when
    # entering a NON-EMPTY container — 101 nested empty arrays
    # parse fine, 101 nested arrays around any element raise.
    # The first violation is always at depth max_nesting + 1, so
    # the reported number equals @depth at the check site.
    def check_nest
      if @max_nesting > 0 && @depth > @max_nesting
        raise NestingError, "nesting of #{@depth} is too deep"
      end
    end

    # NOTE `while true` instead of `loop do` in the container
    # parsers: `loop` costs two extra native frames per nesting
    # level (the method + its block). Under the fattest JIT
    # configs (tier-2 with threshold 1) the ~100-level legal
    # recursion budget overflowed the native stack BEFORE the
    # depth guard could fire (SystemStackError on a 150-deep
    # document). Halving frames-per-level keeps NestingError
    # winning that race in every config.
    def parse_object
      @depth += 1
      @pos += 1
      obj = {}
      skip_ws
      if peek == "}"
        @pos += 1
        @depth -= 1
        return obj
      end
      check_nest
      while true
        skip_ws
        raise ParserError, "expected string key at position #{@pos}" unless peek == "\""
        key_str = parse_string
        key = if @symbolize_names
          key_str.to_sym
        else
          cached = KEY_CACHE[key_str]
          if cached
            cached
          elsif KEY_CACHE.size < KEY_CACHE_CAP
            KEY_CACHE[key_str] = key_str.freeze
          else
            key_str.freeze
          end
        end
        skip_ws
        raise ParserError, "expected ':' at position #{@pos}" unless peek == ":"
        @pos += 1
        skip_ws
        obj[key] = parse_value
        skip_ws
        c = peek
        if c == ","
          @pos += 1
        elsif c == "}"
          @pos += 1
          @depth -= 1
          return obj
        else
          raise ParserError, "expected ',' or '}' at position #{@pos}"
        end
      end
    end

    def parse_array
      @depth += 1
      @pos += 1
      arr = []
      skip_ws
      if peek == "]"
        @pos += 1
        @depth -= 1
        return arr
      end
      check_nest
      while true
        skip_ws
        arr << parse_value
        skip_ws
        c = peek
        if c == ","
          @pos += 1
        elsif c == "]"
          @pos += 1
          @depth -= 1
          return arr
        else
          raise ParserError, "expected ',' or ']' at position #{@pos}"
        end
      end
    end

    def parse_string
      @pos += 1
      out = ""
      while @pos < @len
        c = @chars[@pos]
        if c == "\""
          @pos += 1
          return out
        elsif c == "\\"
          @pos += 1
          esc = @chars[@pos]
          raise ParserError, "dangling backslash at end of input" if esc.nil?
          @pos += 1
          case esc
          when "\"" then out += "\""
          when "\\" then out += "\\"
          when "/" then out += "/"
          when "b" then out += "\b"
          when "f" then out += "\f"
          when "n" then out += "\n"
          when "r" then out += "\r"
          when "t" then out += "\t"
          when "u"
            cp = parse_hex4
            # UTF-16 surrogate handling, matching CRuby (json 2.20,
            # probed): a HIGH surrogate must pair with a following
            # \uXXXX LOW surrogate ("incomplete surrogate pair" /
            # "invalid surrogate pair" ParserErrors otherwise); a
            # LONE LOW surrogate is accepted and emits the code
            # point's raw UTF-8-shaped bytes (WTF-8 — \udc00
            # becomes ED B0 80, an invalid-UTF-8 string like
            # CRuby's).
            if cp >= 0xD800 && cp <= 0xDBFF
              unless @chars[@pos] == "\\" && @chars[@pos + 1] == "u"
                raise ParserError, "incomplete surrogate pair at position #{@pos}"
              end
              pair_pos = @pos
              @pos += 2
              lo = parse_hex4
              unless lo >= 0xDC00 && lo <= 0xDFFF
                raise ParserError, "invalid surrogate pair at position #{pair_pos}"
              end
              cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)
              out += cp.chr(Encoding::UTF_8)
            elsif cp >= 0xDC00 && cp <= 0xDFFF
              out += [0xE0 | (cp >> 12), 0x80 | ((cp >> 6) & 0x3F), 0x80 | (cp & 0x3F)].pack("C3").force_encoding("UTF-8")
            else
              # Encode as UTF-8 (chr(UTF_8) widens the accepted
              # range to U+10FFFF; bare chr only covers 0..255).
              out += cp.chr(Encoding::UTF_8)
            end
          else
            raise ParserError, "bad escape '\\#{esc}' at position #{@pos}"
          end
        elsif c.ord < 0x20
          # Raw control characters are invalid inside JSON strings
          # (CRuby: "invalid ASCII control character in string").
          raise ParserError, "invalid ASCII control character in string at position #{@pos}"
        else
          out += c
          @pos += 1
        end
      end
      raise ParserError, "unterminated string"
    end

    # Read the four hex digits of a \uXXXX escape and advance past them.
    def parse_hex4
      if @pos + 4 > @len
        raise ParserError, "truncated \\u escape at position #{@pos}"
      end
      v = 0
      4.times do
        o = @chars[@pos].ord
        d = if o >= 48 && o <= 57
          o - 48
        elsif o >= 97 && o <= 102
          o - 87
        elsif o >= 65 && o <= 70
          o - 55
        else
          # CRuby: "incomplete unicode character escape sequence" —
          # \uZZZZ must raise, not launder into codepoint 0.
          raise ParserError, "incomplete unicode character escape sequence at position #{@pos}"
        end
        v = v * 16 + d
        @pos += 1
      end
      v
    end

    # CRuby's parse-error rendering (json 2.20 parser.c, probed):
    # "invalid number: '<frag>' at line L column C" — frag is up
    # to 32 chars from the number start, stopping at NUL / space /
    # tab / CR / LF; line and column are 1-based at the number
    # start; an empty frag (number at end of input) renders as
    # bare EOF without quotes.
    def invalid_number(start)
      # Fragment: up to 32 BYTES from the number start (CRuby
      # counts bytes, not characters), stopping at NUL / space /
      # tab / CR / LF — a multibyte char may be cut mid-sequence
      # at the cap. CRuby then strips trailing continuation bytes
      # AND, if the (new) last byte is a lead byte, that too — so
      # a multibyte char ENDING exactly at the 32-byte cap is
      # dropped whole (probed: 30 digits + é keeps only the
      # digits), and one CUT at the cap loses its partial bytes.
      bytes = []
      i = start
      while i < @len && bytes.length < 32
        c = @chars[i]
        break if c == " " || c == "\t" || c == "\r" || c == "\n" || c == "\0"
        c.bytes.each { |b| bytes << b }
        i += 1
      end
      bytes = bytes[0, 32]
      while bytes.length > 0 && bytes[-1] >= 0x80 && bytes[-1] < 0xC0
        bytes.pop
      end
      bytes.pop if bytes.length > 0 && bytes[-1] >= 0xC0
      # Line is newline-counted; column is the 1-based BYTE offset
      # of the number start within its line (CRuby probed:
      # '["éé",01]' reports column 9, not 7).
      line = 1
      col = 1
      j = 0
      while j < start
        c = @chars[j]
        if c == "\n"
          line += 1
          col = 1
        else
          col += c.bytesize
        end
        j += 1
      end
      shown = bytes.empty? ? "EOF" : "'" + bytes.pack("C*").force_encoding("UTF-8") + "'"
      raise ParserError, "invalid number: " + shown + " at line " + line.to_s + " column " + col.to_s
    end

    # Strict JSON number grammar (CRuby probed: leading zeros,
    # bare '-', '1.', '1e', '1e+' all raise "invalid number") +
    # CRuby's exponent-saturation quirks. The serde-backed native
    # path DECLINES any document with a >=19-digit run to this
    # parser, so the grammar here is what the default path
    # enforces for bigint-range documents — it must not be laxer
    # than CRuby.
    def parse_number
      start = @pos
      neg = false
      if @chars[@pos] == "-"
        neg = true
        @pos += 1
      end
      # Integer part: 0 | [1-9][0-9]*
      int_digits = 0
      while @pos < @len && "0123456789".include?(@chars[@pos])
        int_digits += 1
        @pos += 1
      end
      invalid_number(start) if int_digits == 0
      invalid_number(start) if int_digits > 1 && @chars[start + (neg ? 1 : 0)] == "0"
      is_float = false
      frac_digits = 0
      if @pos < @len && @chars[@pos] == "."
        is_float = true
        @pos += 1
        while @pos < @len && "0123456789".include?(@chars[@pos])
          frac_digits += 1
          @pos += 1
        end
        invalid_number(start) if frac_digits == 0
      end
      exp_digits = 0
      exp_neg = false
      abs_exp = 0
      if @pos < @len && (@chars[@pos] == "e" || @chars[@pos] == "E")
        is_float = true
        @pos += 1
        if @pos < @len && (@chars[@pos] == "+" || @chars[@pos] == "-")
          exp_neg = @chars[@pos] == "-"
          @pos += 1
        end
        while @pos < @len && "0123456789".include?(@chars[@pos])
          abs_exp = abs_exp * 10 + (@chars[@pos].ord - 48)
          exp_digits += 1
          @pos += 1
        end
        invalid_number(start) if exp_digits == 0
      end
      if is_float
        # CRuby (json 2.20 parser.c, source-verified + probed): an
        # exponent LITERAL of >= 20 digits (or abs value past i64)
        # saturates the exponent to the i64 extreme of its sign
        # BEFORE the fraction-length adjustment, which then WRAPS
        # in C (i64::MIN - frac_len -> huge positive). Net
        # observable rule, signs following the mantissa:
        #   positive-saturated                  -> ±Infinity
        #   negative-saturated WITH a fraction  -> ±Infinity (wrap)
        #   negative-saturated, no fraction     -> ±0.0
        # So "1e00000000000000000009" is Infinity even though the
        # exponent VALUE is 9, and "1e-00000000000000000009" is 0.0.
        if exp_digits >= 20 || abs_exp > 9223372036854775807
          if !exp_neg || frac_digits > 0
            return neg ? -Float::INFINITY : Float::INFINITY
          else
            return neg ? -0.0 : 0.0
          end
        end
        # Reassemble the slice. Avoid `Array#[start, len]` (not
        # supported on rubyrs); walk the indices.
        buf = ""
        i = start
        while i < @pos
          buf += @chars[i]
          i += 1
        end
        buf.to_f
      else
        buf = ""
        i = start
        while i < @pos
          buf += @chars[i]
          i += 1
        end
        if buf.length <= 17
          # ≤17 chars (sign included) always fits i64.
          buf.to_i
        else
          # rubyrs String#to_i WRAPS past i64 (core gap — tracked
          # separately); Integer arithmetic promotes to Bignum
          # exactly, so fold long literals digit-wise. CRuby parses
          # big JSON integers as exact Integer (probed json 2.20).
          n = 0
          i = neg ? start + 1 : start
          while i < @pos
            n = n * 10 + (@chars[i].ord - 48)
            i += 1
          end
          neg ? -n : n
        end
      end
    end

    def parse_bool
      if @pos + 4 <= @len && @chars[@pos] == "t" && @chars[@pos + 1] == "r" && @chars[@pos + 2] == "u" && @chars[@pos + 3] == "e"
        @pos += 4
        return true
      end
      if @pos + 5 <= @len && @chars[@pos] == "f" && @chars[@pos + 1] == "a" && @chars[@pos + 2] == "l" && @chars[@pos + 3] == "s" && @chars[@pos + 4] == "e"
        @pos += 5
        return false
      end
      raise ParserError, "bad keyword at position #{@pos}"
    end

    def parse_null
      if @pos + 4 <= @len && @chars[@pos] == "n" && @chars[@pos + 1] == "u" && @chars[@pos + 2] == "l" && @chars[@pos + 3] == "l"
        @pos += 4
        return nil
      end
      raise ParserError, "bad keyword at position #{@pos}"
    end
  end

  # ---- Generate ----

  # `opts` is a positional Hash OR a JSON::State (NOT a kwargs
  # splat) to dodge the Ruby-3 trailing-hash auto-coerce: a
  # caller writing `JSON.generate({"a" => 1})` would otherwise
  # see its sole Hash arg eaten as kwargs by rubyrs's call-site
  # lowering, leaving `obj` unbound. The trade is one extra
  # `opts[:key]` lookup per call vs. the deserialisation grenade.
  # Fast path: the no-opts call needs no State at all — building
  # one via state_from_opts (State.new + a 6-key Hash) cost
  # ~3.2 us/call, dominating small-payload generates. Shape notes
  # (each measured on the {} microbench):
  #   - defined CONDITIONALLY at load so the hot path never
  #     re-tests NATIVE_AVAILABLE (a per-call const lookup);
  #   - the host fn returns NIL to signal decline (NaN, custom
  #     objects, non-UTF-8 strings, >100 nesting) instead of
  #     raising, so no begin/rescue frame on the hot path — nil is
  #     unambiguous because generate otherwise always returns a
  #     String. Declines fall through and build the State lazily
  #     for the canon exactly as before.
  if NATIVE_AVAILABLE
    def self.generate(obj, opts = nil)
      if opts.nil?
        r = __rubyrs_json_native_generate(obj)
        return r unless r.nil?
      end
      state = state_from_opts(opts, "", "", "", "", false, MAX_NESTING_DEFAULT)
      generate_from_state(obj, state)
    end
  else
    def self.generate(obj, opts = nil)
      state = state_from_opts(opts, "", "", "", "", false, MAX_NESTING_DEFAULT)
      generate_from_state(obj, state)
    end
  end

  def self.generate_from_state(obj, state)
    # Native fast path for explicit-but-default-compact States:
    # serde_json's emit produces the same compact bytes the canon
    # would, but only for the deterministic subset (Null / Bool /
    # Integer / Float / String / Array / Hash). Custom-object
    # `to_json` overrides OR a State with non-default formatting
    # (indent / space / newlines) needs the pure canon's
    # recursion. The state below is "default compact" iff all
    # four formatting knobs are empty strings. nil from the host
    # fn = decline (see the wrapper above).
    is_default_compact = state.indent.empty? && state.space.empty? && state.object_nl.empty? && state.array_nl.empty?
    if NATIVE_AVAILABLE && is_default_compact && !state.allow_nan? && state.max_nesting == MAX_NESTING_DEFAULT
      r = __rubyrs_json_native_generate(obj)
      return r unless r.nil?
    end
    generate_with(obj, state.indent, state.space, state.object_nl, state.array_nl, state.allow_nan?, state.max_nesting, 0)
  end

  # `JSON.dump` is essentially `JSON.generate` with permissive
  # defaults; CRuby's variant accepts an optional IO + limit
  # arg, but the embedded-host subset narrows to "stringify and
  # return". `allow_nan` defaults true here to mirror CRuby's
  # historical dump behaviour.
  def self.dump(obj)
    generate(obj, { allow_nan: true })
  end

  # CRuby's default pretty formatting: 2-space indent, ": " (no
  # space before the colon, one after), `,\n` between siblings,
  # `\n` after the opening brace/bracket, closing brace/bracket
  # back at the parent's indent level. Empty containers stay
  # compact (`[]` / `{}`). User can override via opts/State.
  def self.pretty_generate(obj, opts = nil)
    state = state_from_opts(opts, "  ", " ", "\n", "\n", false, MAX_NESTING_DEFAULT)
    generate_with(obj, state.indent, state.space, state.object_nl, state.array_nl, state.allow_nan?, state.max_nesting, 0)
  end

  # Normalise `opts` (Hash | JSON::State | nil) into a State
  # whose unset fields fall back to the per-method defaults
  # passed by the caller. Centralises the "Hash or State or
  # nothing" branching so `generate` / `pretty_generate` share
  # one path.
  def self.state_from_opts(opts, def_indent, def_space, def_obj_nl, def_arr_nl, def_allow_nan, def_max_nest)
    return State.new({
      indent: def_indent,
      space: def_space,
      object_nl: def_obj_nl,
      array_nl: def_arr_nl,
      allow_nan: def_allow_nan,
      max_nesting: def_max_nest,
    }) if opts.nil?
    return opts if opts.is_a?(State)
    # Hash path: caller-provided keys override the per-method
    # defaults; missing keys keep the defaults.
    merged = {
      indent: opts.has_key?(:indent) ? opts[:indent] : def_indent,
      space: opts.has_key?(:space) ? opts[:space] : def_space,
      object_nl: opts.has_key?(:object_nl) ? opts[:object_nl] : def_obj_nl,
      array_nl: opts.has_key?(:array_nl) ? opts[:array_nl] : def_arr_nl,
      allow_nan: opts.has_key?(:allow_nan) ? opts[:allow_nan] : def_allow_nan,
      max_nesting: opts.has_key?(:max_nesting) ? opts[:max_nesting] : def_max_nest,
    }
    State.new(merged)
  end

  # Core recursive serializer. `indent` is the per-level indent
  # string (empty in compact mode); `space` is the gap between
  # `:` and value; `obj_nl` / `arr_nl` are the line separators
  # inside object / array bodies. Compact mode passes empty
  # strings throughout, producing the exact byte output CRuby's
  # default `JSON.generate` emits.
  def self.generate_with(obj, indent, space, obj_nl, arr_nl, allow_nan, max_nest, depth)
    case obj
    when nil then "null"
    when true then "true"
    when false then "false"
    when Integer then obj.to_s
    when Float
      if obj.nan? || !obj.infinite?.nil?
        if allow_nan
          # CRuby's JSON.dump emits NaN/Infinity/-Infinity as
          # bare tokens (non-standard JSON, accepted by its own
          # parser). Mirror that surface for `dump`-shape calls.
          return "NaN" if obj.nan?
          return obj > 0 ? "Infinity" : "-Infinity"
        end
        raise GeneratorError, "#{obj} not allowed in JSON"
      end
      float_repr(obj)
    when String then escape_string(obj)
    when Symbol then escape_string(obj.to_s)
    when Array
      # Depth check BEFORE the empty-container early-return —
      # CRuby raises for an empty array at depth 101 too.
      if max_nest > 0 && depth + 1 > max_nest
        # CRuby (json 2.20, probed): generate allows depth ≤
        # max_nesting, and the raise text pins the NUMBER at
        # max_nesting (not the actual depth) + carries the
        # circular-reference hint suffix.
        raise NestingError, "nesting of #{max_nest} is too deep. Did you try to serialize objects with circular references?"
      end
      return "[]" if obj.empty?
      inner_indent = indent * (depth + 1)
      outer_indent = indent * depth
      parts = []
      obj.each { |v| parts << inner_indent + generate_with(v, indent, space, obj_nl, arr_nl, allow_nan, max_nest, depth + 1) }
      "[" + arr_nl + parts.join("," + arr_nl) + arr_nl + outer_indent + "]"
    when Hash
      if max_nest > 0 && depth + 1 > max_nest
        # See the Array arm — pinned number + hint suffix.
        raise NestingError, "nesting of #{max_nest} is too deep. Did you try to serialize objects with circular references?"
      end
      return "{}" if obj.empty?
      inner_indent = indent * (depth + 1)
      outer_indent = indent * depth
      parts = []
      obj.each do |k, v|
        # CRuby's JSON.generate stringifies non-String keys via
        # to_s before emitting (Symbol → its name; Integer →
        # its decimal repr). We mirror that here.
        key_s = k.is_a?(String) ? k : k.to_s
        parts << inner_indent + escape_string(key_s) + ":" + space + generate_with(v, indent, space, obj_nl, arr_nl, allow_nan, max_nest, depth + 1)
      end
      "{" + obj_nl + parts.join("," + obj_nl) + obj_nl + outer_indent + "}"
    else
      raise GeneratorError, "cannot generate JSON from #{obj.class}"
    end
  end

  # CRuby's json gem does NOT render Float values with Float#to_s —
  # its generator runs fpconv (Grisu2 shortest digits + fpconv's own
  # fixed/scientific window): `1e15` → "1e+15" (to_s: "1.0e+15"),
  # `1.5e-5` → "0.000015" (to_s: "1.5e-05"), `5e-324` → "5e-324".
  # rubyrs registers the exact fpconv port as the always-on
  # `__rubyrs_json_float_repr` host fn (json_float.rs); embedders
  # that skip host-fn registration fall back to a pure-Ruby reshape
  # of Float#to_s into the same layout. The fallback matches fpconv
  # except on the rare doubles where Grisu2 picks different shortest
  # digits than Float#to_s's Ryū (both round-trip; e.g. CRuby json
  # emits 1234567890123456.7 where to_s says ...6.8).
  HAVE_NATIVE_FLOAT_REPR = defined?(__rubyrs_json_float_repr) ? true : false

  def self.float_repr(f)
    return __rubyrs_json_float_repr(f) if HAVE_NATIVE_FLOAT_REPR
    s = f.to_s
    neg = s.start_with?("-")
    s = s[1..-1] if neg
    mant, es = s.split("e")
    e10 = es ? es.to_i : 0
    int_part, frac_part = mant.split(".")
    frac_part = "" if frac_part.nil?
    raw = int_part + frac_part
    point = int_part.length + e10   # digits before the decimal point
    # First significant digit; all-zero means ±0.0.
    lead = 0
    lead += 1 while lead < raw.length && raw[lead] == "0"
    return neg ? "-0.0" : "0.0" if lead == raw.length
    digits = raw[lead..-1]
    tail = digits.length
    tail -= 1 while tail > 1 && digits[tail - 1] == "0"
    digits = digits[0, tail]
    decpt = point - lead
    k = decpt - digits.length
    exp10 = (decpt - 1).abs
    body = if k >= 0 && exp10 < 15
      digits + ("0" * k) + ".0"
    elsif k < 0 && (k > -7 || exp10 < 10)
      if decpt <= 0
        "0." + ("0" * (-decpt)) + digits
      else
        digits[0, decpt] + "." + digits[decpt..-1]
      end
    else
      m = digits.length > 1 ? digits[0, 1] + "." + digits[1..-1] : digits
      m + "e" + (decpt - 1 < 0 ? "-" : "+") + exp10.to_s
    end
    neg ? "-" + body : body
  end

  def self.escape_string(s)
    # CRuby's generator (json 2.20, probed) validates encodings:
    #   - BINARY (ASCII-8BIT) with non-ASCII bytes: if the bytes
    #     happen to be valid UTF-8, emit them (CRuby warns that
    #     json 3.0 will raise; warning skipped — stderr-only and
    #     its wording ties to CRuby internals); otherwise raise
    #     GeneratorError '"\xNN" from ASCII-8BIT to UTF-8' naming
    #     the first non-ASCII byte.
    #   - any other encoding with malformed UTF-8 content: raise
    #     GeneratorError "source sequence is illegal/malformed
    #     utf-8".
    if s.encoding == Encoding::ASCII_8BIT
      unless s.ascii_only?
        t = s.dup
        t.force_encoding("UTF-8")
        unless t.valid_encoding?
          b = s.bytes.find { |x| x >= 0x80 }
          raise GeneratorError, "\"\\x#{sprintf("%02X", b)}\" from ASCII-8BIT to UTF-8"
        end
      end
    elsif !s.valid_encoding?
      raise GeneratorError, "source sequence is illegal/malformed utf-8"
    end
    out = "\""
    s.chars.each do |c|
      case c
      when "\"" then out += "\\\""
      when "\\" then out += "\\\\"
      when "\b" then out += "\\b"
      when "\f" then out += "\\f"
      when "\n" then out += "\\n"
      when "\r" then out += "\\r"
      when "\t" then out += "\\t"
      else
        b = c.bytes[0]
        if b < 0x20
          out += "\\u" + sprintf("%04x", b)
        else
          out += c
        end
      end
    end
    out + "\""
  end
end

# `to_json` mixin on the basic types. CRuby's `json` gem
# registers these by re-opening each class; mirroring that lets
# `42.to_json`, `[1,2].to_json`, `{a:1}.to_json` work without
# the caller having to spell `JSON.generate(...)`. Each method
# takes an optional state arg for API parity but the embedded
# canon ignores it (state-driven formatting is the
# `pretty_generate` knobs which we expose by their own method
# names).

# `to_json` mixin — each method takes an optional first `state`
# arg (Hash or JSON::State or nil). Collections forward it to
# JSON.generate so `arr.to_json(JSON::State.new(indent: "  "))`
# emits pretty form; primitives ignore it (formatting state has
# nothing to do with how `42` or `"x"` renders).

class NilClass
  def to_json(*)
    "null"
  end
end

class TrueClass
  def to_json(*)
    "true"
  end
end

class FalseClass
  def to_json(*)
    "false"
  end
end

class Integer
  def to_json(*)
    to_s
  end
end

class Float
  def to_json(*)
    JSON.generate(self)
  end
end

class String
  def to_json(*)
    JSON.escape_string(self)
  end
end

class Symbol
  def to_json(*)
    JSON.escape_string(to_s)
  end
end

class Array
  def to_json(state = nil, *)
    JSON.generate(self, state)
  end
end

class Hash
  def to_json(state = nil, *)
    JSON.generate(self, state)
  end
end

# `Object#to_json` fall-through — anything outside the basic
# types above stringifies via `to_s` and wraps in JSON quotes.
# Matches CRuby's vanilla `json` gem behaviour
# (`Object.new.to_json` → `"\"#<Object:0x...>\""`). Lets user
# code do `[obj1, obj2].to_json` without raising on objects
# whose JSON shape is "just stringify me."
class Object
  def to_json(*)
    JSON.escape_string(to_s)
  end
end

# `as_json` convention — ActiveSupport-shape Object→JSON-friendly-
# value coercion. Vanilla CRuby `json` does NOT define this; rubyrs
# exposes it as forward-compat so user code that pre-normalises via
# `.as_json` (a common Rails-adjacent idiom) works the same on both
# runtimes. Cross-runtime fixture deliberately avoids touching this
# surface — it's a rubyrs-side affordance, not a parity claim.
#
# Per-class semantics match ActiveSupport's `to_json/as_json`
# split: primitives return self; collections recurse; Symbols
# stringify; Object#as_json falls through to to_s.

class NilClass
  def as_json(*)
    nil
  end
end

class TrueClass
  def as_json(*)
    true
  end
end

class FalseClass
  def as_json(*)
    false
  end
end

class Integer
  def as_json(*)
    self
  end
end

class Float
  def as_json(*)
    self
  end
end

class String
  def as_json(*)
    self
  end
end

class Symbol
  def as_json(*)
    to_s
  end
end

class Array
  def as_json(*)
    map { |v| v.as_json }
  end
end

class Hash
  def as_json(*)
    out = {}
    each { |k, v| out[k.is_a?(Symbol) ? k.to_s : k] = v.as_json }
    out
  end
end

class Object
  # Default fallback: ActiveSupport's convention is `to_s`. The
  # blessed-gem-menu ActiveSupport-lite (menu item 3) is allowed
  # to override this with the full Rails-adjacent behaviour
  # without breaking rubyrs's existing JSON surface.
  def as_json(*)
    to_s
  end
end
