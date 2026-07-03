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
  class NestingError < JSONError; end

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
      rescue RuntimeError => e
        # Contract mirror of the option-path rescue below: depth
        # overflows surface as NestingError, non-UTF-8 input falls
        # back to the pure canon (CRuby's parser accepts raw bytes;
        # serde_json requires UTF-8), anything else is a genuine
        # syntax error -> ParserError.
        if e.message.include?("recursion limit") || e.message.include?("too deep")
          raise NestingError, e.message
        end
        unless e.message.include?("non-utf8")
          raise ParserError, e.message
        end
        # fall through to the pure canon
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
      rescue RuntimeError => e
        # Depth overflow ("nesting of N is too deep" from the
        # visitor's own depth guard, or serde's "recursion limit
        # exceeded" backstop) is the condition the canon's
        # `enter_nest` raises `JSON::NestingError` for. Re-raise
        # as the documented class so user rescue clauses see the
        # contract surface. Non-UTF-8 input falls back to the
        # pure canon (CRuby's parser accepts raw bytes; serde_json
        # requires UTF-8). Other parse errors map to the generic
        # `JSON::ParserError`.
        if e.message.include?("recursion limit") || e.message.include?("too deep")
          raise NestingError, e.message
        end
        unless e.message.include?("non-utf8")
          raise ParserError, e.message
        end
        # fall through to the pure canon
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
    def initialize(str, symbolize_names = false, max_nesting = MAX_NESTING_DEFAULT, allow_nan = false)
      @chars = str.chars
      @len = @chars.length
      @pos = 0
      @symbolize_names = symbolize_names
      @max_nesting = max_nesting
      @allow_nan = allow_nan
      @depth = 0
    end

    def enter_nest
      @depth += 1
      if @max_nesting > 0 && @depth > @max_nesting
        raise NestingError, "nesting of #{@depth} is too deep"
      end
    end

    def leave_nest
      @depth -= 1
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

    def parse_object
      enter_nest
      @pos += 1
      obj = {}
      skip_ws
      if peek == "}"
        @pos += 1
        leave_nest
        return obj
      end
      loop do
        skip_ws
        raise ParserError, "expected string key at position #{@pos}" unless peek == "\""
        key_str = parse_string
        key = @symbolize_names ? key_str.to_sym : key_str
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
          leave_nest
          return obj
        else
          raise ParserError, "expected ',' or '}' at position #{@pos}"
        end
      end
    end

    def parse_array
      enter_nest
      @pos += 1
      arr = []
      skip_ws
      if peek == "]"
        @pos += 1
        leave_nest
        return arr
      end
      loop do
        skip_ws
        arr << parse_value
        skip_ws
        c = peek
        if c == ","
          @pos += 1
        elsif c == "]"
          @pos += 1
          leave_nest
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
            # Combine a UTF-16 surrogate pair into one astral code
            # point, matching CRuby: a high surrogate (D800..DBFF)
            # must be followed by a \uXXXX low surrogate (DC00..DFFF).
            if cp >= 0xD800 && cp <= 0xDBFF &&
               @chars[@pos] == "\\" && @chars[@pos + 1] == "u"
              @pos += 2
              lo = parse_hex4
              if lo >= 0xDC00 && lo <= 0xDFFF
                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00)
              else
                # Not a valid low surrogate: emit the high surrogate's
                # code point and let `lo` fall through as its own char.
                out += cp.chr(Encoding::UTF_8)
                cp = lo
              end
            end
            # Encode the code point as UTF-8 (chr(UTF_8) widens the
            # accepted range to U+10FFFF; bare chr only covers 0..255).
            out += cp.chr(Encoding::UTF_8)
          else
            raise ParserError, "bad escape '\\#{esc}' at position #{@pos}"
          end
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
      hex = @chars[@pos] + @chars[@pos + 1] + @chars[@pos + 2] + @chars[@pos + 3]
      @pos += 4
      hex.to_i(16)
    end

    def parse_number
      start = @pos
      @pos += 1 if @chars[@pos] == "-"
      while @pos < @len && "0123456789".include?(@chars[@pos])
        @pos += 1
      end
      is_float = false
      if @pos < @len && @chars[@pos] == "."
        is_float = true
        @pos += 1
        while @pos < @len && "0123456789".include?(@chars[@pos])
          @pos += 1
        end
      end
      if @pos < @len && (@chars[@pos] == "e" || @chars[@pos] == "E")
        is_float = true
        @pos += 1
        if @pos < @len && (@chars[@pos] == "+" || @chars[@pos] == "-")
          @pos += 1
        end
        while @pos < @len && "0123456789".include?(@chars[@pos])
          @pos += 1
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
      is_float ? buf.to_f : buf.to_i
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
  def self.generate(obj, opts = nil)
    # Fast path: the no-opts call needs no State at all — building
    # one via state_from_opts (State.new + a 6-key Hash) cost
    # ~3.2 us/call, dominating small-payload generates. The native
    # emitter IS the default-compact form; on decline (NaN, custom
    # objects, non-UTF-8 strings, >100 nesting) fall through and
    # build the State lazily for the canon exactly as before.
    if opts.nil? && NATIVE_AVAILABLE
      begin
        return __rubyrs_json_native_generate(obj)
      rescue RuntimeError
        # fall through to the canon path below
      end
    end
    state = state_from_opts(opts, "", "", "", "", false, MAX_NESTING_DEFAULT)
    # Native fast path: serde_json's emit produces the same
    # compact bytes the canon would, but only for the
    # deterministic subset (Null / Bool / Integer / Float /
    # String / Array / Hash). Custom-object `to_json` overrides
    # OR a State with non-default formatting (indent / space /
    # newlines) needs the pure canon's recursion. The state
    # below is "default compact" iff all four formatting knobs
    # are empty strings.
    is_default_compact = state.indent.empty? && state.space.empty? && state.object_nl.empty? && state.array_nl.empty?
    if NATIVE_AVAILABLE && is_default_compact && !state.allow_nan? && state.max_nesting == MAX_NESTING_DEFAULT
      begin
        return __rubyrs_json_native_generate(obj)
      rescue RuntimeError => e
        # Native bailed (NaN, custom Object, unsupported value
        # — see json_native.rs's `write_value` fall-through).
        # Re-run on the pure canon which has full Object#to_json
        # / NaN-with-allow_nan / nested-mixin coverage.
      end
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
      return "[]" if obj.empty?
      if max_nest > 0 && depth + 1 > max_nest
        raise NestingError, "nesting of #{depth + 1} is too deep"
      end
      inner_indent = indent * (depth + 1)
      outer_indent = indent * depth
      parts = []
      obj.each { |v| parts << inner_indent + generate_with(v, indent, space, obj_nl, arr_nl, allow_nan, max_nest, depth + 1) }
      "[" + arr_nl + parts.join("," + arr_nl) + arr_nl + outer_indent + "]"
    when Hash
      return "{}" if obj.empty?
      if max_nest > 0 && depth + 1 > max_nest
        raise NestingError, "nesting of #{depth + 1} is too deep"
      end
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
