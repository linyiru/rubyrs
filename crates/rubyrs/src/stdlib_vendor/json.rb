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
#   - `JSON.pretty_generate`, `JSON.dump`, `JSON.load`, `to_json`
#     mixin on basic types
#   - Options: `allow_nan`, `max_nesting`, `symbolize_names`,
#     `quirks_mode`, `object_class`
#   - `JSON::Ext::Parser` / `JSON::Ext::Generator` (the C-ext
#     classes — flori-json-cext already covers that surface in
#     the `examples/` directory)
#   - Unicode surrogate-pair decoding in `\uXXXX` escapes —
#     non-BMP characters in JSON input outside the parser's
#     scope here. Class-`h` divergence per ADR 0019.
#
# Float divergence (ADR 0019 class `h`): both runtimes use
# Ruby's `Float#to_s`, so round-trip behaviour matches CRuby
# wherever `Float#to_s` does. Inputs at the edge of IEEE-754
# precision may diverge in the last digit between
# implementations; that's an accepted Rule 6 deviation.

module JSON
  class ParserError < StandardError; end
  class GeneratorError < StandardError; end

  # ---- Parse ----

  def self.parse(str, opts = nil)
    raise ParserError, "input must be a String" unless str.is_a?(String)
    symbolize = opts && opts[:symbolize_names] ? true : false
    p = Parser.new(str, symbolize)
    p.parse_top
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

  class Parser
    def initialize(str, symbolize_names = false)
      @chars = str.chars
      @len = @chars.length
      @pos = 0
      @symbolize_names = symbolize_names
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
      when "n" then parse_null
      else
        if c == "-" || "0123456789".include?(c)
          parse_number
        else
          raise ParserError, "unexpected character '#{c}' at position #{@pos}"
        end
      end
    end

    def parse_object
      @pos += 1
      obj = {}
      skip_ws
      if peek == "}"
        @pos += 1
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
          return obj
        else
          raise ParserError, "expected ',' or '}' at position #{@pos}"
        end
      end
    end

    def parse_array
      @pos += 1
      arr = []
      skip_ws
      if peek == "]"
        @pos += 1
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
            if @pos + 4 > @len
              raise ParserError, "truncated \\u escape at position #{@pos}"
            end
            hex = @chars[@pos] + @chars[@pos + 1] + @chars[@pos + 2] + @chars[@pos + 3]
            @pos += 4
            cp = hex.to_i(16)
            # Surrogate-pair decoding is a deferred extension
            # (see file header); we emit the code point as-is.
            # ASCII range comes out byte-clean; non-BMP code
            # points get the Ruby chr behaviour for that
            # codepoint (which encodes UTF-8 above U+007F when
            # the host String encoding permits).
            out += cp.chr
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

  # `opts` is a positional Hash (NOT a kwargs splat) to dodge
  # the Ruby-3 trailing-hash auto-coerce: a caller writing
  # `JSON.generate({"a" => 1})` would otherwise see its sole
  # Hash arg eaten as kwargs by rubyrs's call-site lowering,
  # leaving `obj` unbound. The trade is one extra `opts[:key]`
  # lookup per call vs. the deserialisation grenade.
  def self.generate(obj, opts = nil)
    allow_nan = opts && opts[:allow_nan]
    generate_with(obj, "", "", "", "", allow_nan ? true : false)
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
  # compact (`[]` / `{}`).
  def self.pretty_generate(obj)
    generate_with(obj, "  ", " ", "\n", "\n", false)
  end

  # Core recursive serializer. `indent` is the per-level indent
  # string (empty in compact mode); `space` is the gap between
  # `:` and value; `obj_nl` / `arr_nl` are the line separators
  # inside object / array bodies. Compact mode passes empty
  # strings throughout, producing the exact byte output CRuby's
  # default `JSON.generate` emits.
  def self.generate_with(obj, indent, space, obj_nl, arr_nl, allow_nan, depth = 0)
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
      obj.to_s
    when String then escape_string(obj)
    when Symbol then escape_string(obj.to_s)
    when Array
      return "[]" if obj.empty?
      inner_indent = indent * (depth + 1)
      outer_indent = indent * depth
      parts = []
      obj.each { |v| parts << inner_indent + generate_with(v, indent, space, obj_nl, arr_nl, allow_nan, depth + 1) }
      "[" + arr_nl + parts.join("," + arr_nl) + arr_nl + outer_indent + "]"
    when Hash
      return "{}" if obj.empty?
      inner_indent = indent * (depth + 1)
      outer_indent = indent * depth
      parts = []
      obj.each do |k, v|
        # CRuby's JSON.generate stringifies non-String keys via
        # to_s before emitting (Symbol → its name; Integer →
        # its decimal repr). We mirror that here.
        key_s = k.is_a?(String) ? k : k.to_s
        parts << inner_indent + escape_string(key_s) + ":" + space + generate_with(v, indent, space, obj_nl, arr_nl, allow_nan, depth + 1)
      end
      "{" + obj_nl + parts.join("," + obj_nl) + obj_nl + outer_indent + "}"
    else
      raise GeneratorError, "cannot generate JSON from #{obj.class}"
    end
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
    self.to_s
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
    JSON.escape_string(self.to_s)
  end
end

class Array
  def to_json(*)
    JSON.generate(self)
  end
end

class Hash
  def to_json(*)
    JSON.generate(self)
  end
end
