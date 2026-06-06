# Focused pure-Ruby YAML loader for the Jekyll/Sinatra front-matter
# and config subset (ADR 0026 omakase blessed-reimpl). NOT a full
# YAML 1.1/1.2 implementation — covers block mappings, block
# sequences, typed scalars, single/double-quoted strings, flow
# `[..]`/`{..}`, comments, and `---`/`...` document markers.
#
# Reopens the `YAML` module shell the require path already installed
# (so `YAML`/`Psych` — the same object — gain `.load`/`.safe_load`/
# `.load_file`). `SafeYAML.load` / `.load_file` delegate here so
# safe_yaml's Psych-handler internals are bypassed entirely.
#
# Discovery: P3 Jekyll spike — jekyll reads front-matter via
# `SafeYAML.load` / `SafeYAML.load_file`; safe_yaml itself subclasses
# `Psych::Handler` and delegates to real Psych, which rubyrs lacks.

module YAML
  class << self
    def load(source, *_args, **_opts)
      RubyrsYAMLParse.parse_document(source)
    end
    alias_method :safe_load, :load
    alias_method :unsafe_load, :load
    alias_method :parse, :load

    def load_file(path, *_args, **_opts)
      # Call the parser directly rather than bare `load` — inside this
      # singleton method a bare `load` can resolve to Kernel#load (the
      # file loader) instead of YAML.load.
      RubyrsYAMLParse.parse_document(File.read(path))
    end
  end
end

module SafeYAML
  OPTIONS = {} unless defined?(OPTIONS)

  class << self
    def load(source, *_args, **_opts)
      RubyrsYAMLParse.parse_document(source)
    end

    def load_file(path, *_args, **_opts)
      # Call the parser directly rather than bare `load` — inside this
      # singleton method a bare `load` can resolve to Kernel#load (the
      # file loader) instead of YAML.load.
      RubyrsYAMLParse.parse_document(File.read(path))
    end
  end
end

# Implementation namespace kept out of YAML/SafeYAML so reopening
# those modules elsewhere can't clobber the parser internals.
module RubyrsYAMLParse
  module_function

  def parse_document(source)
    return nil if source.nil?
    lines = preprocess(source.to_s)
    return nil if lines.empty?
    parse_block(lines, [0], 0)
  end

  # Strip a leading `---`, stop at a trailing `...`/`---`, drop blank
  # and comment-only lines, strip trailing comments. Returns
  # [[indent, content], ...].
  def preprocess(src)
    out = []
    src.split("\n", -1).each do |line|
      stripped = line.strip
      next if stripped == "---"
      break if stripped == "..."
      next if stripped.empty?
      next if stripped.start_with?("#")
      content = strip_trailing_comment(line)
      next if content.strip.empty?
      indent = content[/\A */].length
      out << [indent, content.strip]
    end
    out
  end

  def strip_trailing_comment(line)
    in_single = false
    in_double = false
    i = 0
    while i < line.length
      c = line[i]
      if c == "'" && !in_double
        in_single = !in_single
      elsif c == '"' && !in_single
        in_double = !in_double
      elsif c == "#" && !in_single && !in_double
        prev = i > 0 ? line[i - 1] : " "
        return line[0...i] if prev == " " || prev == "\t"
      end
      i += 1
    end
    line
  end

  def parse_block(lines, idx, min_indent)
    return nil if idx[0] >= lines.length
    indent, content = lines[idx[0]]
    return nil if indent < min_indent
    if content.start_with?("- ") || content == "-"
      parse_sequence(lines, idx, indent)
    elsif split_key(content)
      parse_mapping(lines, idx, indent)
    else
      idx[0] += 1
      scalar(content)
    end
  end

  def parse_mapping(lines, idx, indent)
    map = {}
    while idx[0] < lines.length
      cur_indent, content = lines[idx[0]]
      break if cur_indent < indent
      break if cur_indent > indent
      kv = split_key(content)
      break if kv.nil?
      key, rest = kv
      idx[0] += 1
      if rest.empty?
        if idx[0] < lines.length && lines[idx[0]][0] > indent
          map[scalar(key)] = parse_block(lines, idx, indent + 1)
        elsif idx[0] < lines.length && lines[idx[0]][0] == indent &&
              (lines[idx[0]][1].start_with?("- ") || lines[idx[0]][1] == "-")
          map[scalar(key)] = parse_sequence(lines, idx, indent)
        else
          map[scalar(key)] = nil
        end
      else
        map[scalar(key)] = scalar(rest)
      end
    end
    map
  end

  def parse_sequence(lines, idx, indent)
    seq = []
    while idx[0] < lines.length
      cur_indent, content = lines[idx[0]]
      break if cur_indent < indent
      break unless content.start_with?("- ") || content == "-"
      item = content == "-" ? "" : content[2..]
      idx[0] += 1
      if item.strip.empty?
        if idx[0] < lines.length && lines[idx[0]][0] > indent
          seq << parse_block(lines, idx, indent + 1)
        else
          seq << nil
        end
      elsif (kv = split_key(item))
        key, rest = kv
        m = {}
        m[scalar(key)] = rest.empty? ? nil : scalar(rest)
        item_indent = indent + 2
        while idx[0] < lines.length && lines[idx[0]][0] >= item_indent &&
              (ikv = split_key(lines[idx[0]][1]))
          ik, ir = ikv
          idx[0] += 1
          m[scalar(ik)] = ir.empty? ? nil : scalar(ir)
        end
        seq << m
      else
        seq << scalar(item)
      end
    end
    seq
  end

  # Find the top-level `key:` split (colon + space or EOL), respecting
  # quotes and flow brackets. Returns [key, rest] or nil.
  def split_key(content)
    in_single = false
    in_double = false
    depth = 0
    i = 0
    while i < content.length
      c = content[i]
      if c == "'" && !in_double
        in_single = !in_single
      elsif c == '"' && !in_single
        in_double = !in_double
      elsif !in_single && !in_double
        if c == "[" || c == "{"
          depth += 1
        elsif c == "]" || c == "}"
          depth -= 1
        elsif c == ":" && depth == 0
          nxt = i + 1 < content.length ? content[i + 1] : " "
          if nxt == " " || i + 1 == content.length
            return [content[0...i].strip, content[(i + 1)..].to_s.strip]
          end
        end
      end
      i += 1
    end
    nil
  end

  def scalar(str)
    s = str.strip
    return nil if s.empty? || s == "~" || s == "null" || s == "Null" || s == "NULL"
    if s.start_with?('"')
      parse_double_quoted(s)
    elsif s.start_with?("'")
      parse_single_quoted(s)
    elsif s.start_with?("[")
      parse_flow_seq(s)
    elsif s.start_with?("{")
      parse_flow_map(s)
    else
      case s
      when "true", "True", "TRUE" then true
      when "false", "False", "FALSE" then false
      when /\A[-+]?\d+\z/ then s.to_i
      when /\A[-+]?\d+\.\d+\z/ then s.to_f
      when /\A[-+]?\d+(\.\d+)?[eE][-+]?\d+\z/ then s.to_f
      else s
      end
    end
  end

  def parse_double_quoted(s)
    inner = s[1..-2].to_s
    out = +""
    i = 0
    while i < inner.length
      c = inner[i]
      if c == "\\" && i + 1 < inner.length
        n = inner[i + 1]
        out << case n
               when "n" then "\n"
               when "t" then "\t"
               when "r" then "\r"
               when '"' then '"'
               when "\\" then "\\"
               when "0" then "\0"
               else n
               end
        i += 2
      else
        out << c
        i += 1
      end
    end
    out
  end

  def parse_single_quoted(s)
    s[1..-2].to_s.gsub("''", "'")
  end

  def parse_flow_seq(s)
    inner = s[1..-2].to_s.strip
    return [] if inner.empty?
    split_flow(inner).map { |e| scalar(e) }
  end

  def parse_flow_map(s)
    inner = s[1..-2].to_s.strip
    m = {}
    return m if inner.empty?
    split_flow(inner).each do |pair|
      kv = split_key(pair)
      m[scalar(kv[0])] = kv[1].empty? ? nil : scalar(kv[1]) if kv
    end
    m
  end

  def split_flow(s)
    parts = []
    depth = 0
    in_single = false
    in_double = false
    start = 0
    i = 0
    while i < s.length
      c = s[i]
      if c == "'" && !in_double
        in_single = !in_single
      elsif c == '"' && !in_single
        in_double = !in_double
      elsif !in_single && !in_double
        if c == "[" || c == "{"
          depth += 1
        elsif c == "]" || c == "}"
          depth -= 1
        elsif c == "," && depth == 0
          parts << s[start...i].strip
          start = i + 1
        end
      end
      i += 1
    end
    parts << s[start..].strip
    parts
  end
end
