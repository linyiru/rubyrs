# Psych streaming-parser internals — the event-driven layer the
# focused YAML loader (yaml.rb) deliberately skipped. Concatenated
# AFTER yaml.rb (so `RubyrsYAMLParse`'s scalar/quote/flow helpers are
# already defined) into the same `require "yaml"` / `"psych"` source.
#
# Driver: full `require "rubocop"`. RuboCop's YAMLDuplicationChecker
# (config_loader.rb, run for every .rubocop.yml) does:
#   handler = DuplicationCheckHandler.new(&on_duplicated)   # < Psych::TreeBuilder
#   parser  = Psych::Parser.new(handler)
#   parser.parse(yaml_string, filename)                      # drives handler events
#   parser.handler.root.children[0]
# and DuplicationCheckHandler overrides `end_mapping`, calling `super`
# and inspecting `mapping_node.children.each_slice(2)` for duplicate
# keys (`key.value`, `key.start_line`). That is a streaming-handler
# pattern: it intercepts parse EVENTS, so the materializing
# `YAML.load` path can't satisfy it — we need a real
# Parser→Handler→TreeBuilder→Nodes pipeline.
#
# Scope (phase 1): exactly what RuboCop exercises. `YAML.load` keeps
# its existing materializing path (yaml.rb); this file only adds the
# event flow. NOT full Psych — no anchors/aliases, explicit tags,
# multi-document streams, or block scalars (|, >); node `value`s are
# the raw (unquoted) strings, with type coercion left to a ToRuby
# visitor that isn't needed here. The parser mirrors `RubyrsYAMLParse`
# structurally but emits events and retains 0-based source line
# numbers (RuboCop needs `key.start_line`, which the loader's
# `preprocess` discards).

module Psych
  VERSION = "5.1.2" unless defined?(Psych::VERSION)

  # ---- node tree -------------------------------------------------
  module Nodes
    class Node
      attr_accessor :children
      def initialize
        @children = []
      end
    end

    class Stream < Node
      attr_accessor :encoding
      def initialize(encoding = nil)
        super()
        @encoding = encoding
      end
    end

    class Document < Node
      attr_accessor :version, :tag_directives, :implicit, :implicit_end
      def initialize(version = [], tag_directives = [], implicit = true)
        super()
        @version = version
        @tag_directives = tag_directives
        @implicit = implicit
        @implicit_end = true
      end

      def root
        children.first
      end
    end

    class Mapping < Node
      ANY = 0
      BLOCK = 1
      FLOW = 2
      attr_accessor :anchor, :tag, :implicit, :style,
                    :start_line, :end_line, :start_column, :end_column
      def initialize(anchor = nil, tag = nil, implicit = true, style = BLOCK)
        super()
        @anchor = anchor
        @tag = tag
        @implicit = implicit
        @style = style
      end
    end

    class Sequence < Node
      ANY = 0
      BLOCK = 1
      FLOW = 2
      attr_accessor :anchor, :tag, :implicit, :style,
                    :start_line, :end_line, :start_column, :end_column
      def initialize(anchor = nil, tag = nil, implicit = true, style = BLOCK)
        super()
        @anchor = anchor
        @tag = tag
        @implicit = implicit
        @style = style
      end
    end

    class Scalar < Node
      ANY = 0
      PLAIN = 1
      SINGLE_QUOTED = 2
      DOUBLE_QUOTED = 3
      LITERAL = 4
      FOLDED = 5
      attr_accessor :value, :anchor, :tag, :plain, :quoted, :style,
                    :start_line, :end_line, :start_column, :end_column
      def initialize(value = "", anchor = nil, tag = nil,
                     plain = true, quoted = false, style = ANY)
        super()
        @value = value
        @anchor = anchor
        @tag = tag
        @plain = plain
        @quoted = quoted
        @style = style
      end
    end

    class Alias < Node
      attr_accessor :anchor
      def initialize(anchor = nil)
        super()
        @anchor = anchor
      end
    end
  end

  # ---- handler / tree builder ------------------------------------
  #
  # The no-op event sink subclasses override selectively. Signatures
  # match CRuby's Psych::Handler so a subclass written against real
  # Psych (RuboCop's) works unchanged.
  class Handler
    def start_stream(encoding); end
    def end_stream; end
    def start_document(version, tag_directives, implicit); end
    def end_document(implicit_end = false); end
    def alias(anchor); end
    def scalar(value, anchor, tag, plain, quoted, style); end
    def start_sequence(anchor, tag, implicit, style); end
    def end_sequence; end
    def start_mapping(anchor, tag, implicit, style); end
    def end_mapping; end
    def empty; end
    def streaming?
      false
    end
  end

  # Builds a Nodes tree from the event stream. Each end_* returns the
  # node it closed (RuboCop's DuplicationCheckHandler relies on
  # `mapping_node = super` inside its `end_mapping`).
  class TreeBuilder < Handler
    attr_reader :root

    def initialize
      @stack = []
      @last = nil
      @root = nil
    end

    def start_stream(encoding)
      @root = Nodes::Stream.new(encoding)
      push(@root)
    end

    def end_stream
      pop
    end

    def start_document(version, tag_directives, implicit)
      n = Nodes::Document.new(version, tag_directives, implicit)
      @last.children << n
      push(n)
    end

    def end_document(implicit_end = false)
      @last.implicit_end = implicit_end
      pop
    end

    def start_mapping(anchor, tag, implicit, style)
      n = Nodes::Mapping.new(anchor, tag, implicit, style)
      @last.children << n
      push(n)
    end

    def end_mapping
      pop
    end

    def start_sequence(anchor, tag, implicit, style)
      n = Nodes::Sequence.new(anchor, tag, implicit, style)
      @last.children << n
      push(n)
    end

    def end_sequence
      pop
    end

    def scalar(value, anchor, tag, plain, quoted, style)
      s = Nodes::Scalar.new(value, anchor, tag, plain, quoted, style)
      @last.children << s
      s
    end

    def alias(anchor)
      a = Nodes::Alias.new(anchor)
      @last.children << a
      a
    end

    private

    def push(value)
      @stack.push(value)
      @last = value
      value
    end

    def pop
      node = @stack.pop
      @last = @stack.last
      node
    end
  end

  # ---- the event-emitting parser ---------------------------------
  #
  # Structurally a port of `RubyrsYAMLParse` (block mapping / block
  # sequence / scalar / single+double quoted / flow `[]`/`{}` /
  # comments / `---`,`...` markers), but it emits Psych events instead
  # of materializing objects, and keeps the original 0-based line
  # number on every line so scalar nodes carry `start_line`. Scalar
  # node values are the raw (unquoted) strings — no int/bool/float
  # coercion (that belongs in a ToRuby visitor, which this layer
  # doesn't need). Reuses RubyrsYAMLParse's split_key / quote / flow
  # helpers.
  class Parser
    UTF8 = 1
    UTF16LE = 2
    UTF16BE = 3

    attr_reader :handler

    def initialize(handler = TreeBuilder.new)
      @handler = handler
    end

    def parse(yaml, path = nil)
      @path = path
      # Pre-computed block-scalar (`|`/`>`) values, indexed by the
      # sentinel preprocess_with_lines leaves in the line stream and
      # emit_scalar swaps back in.
      @block_scalars = []
      @handler.start_stream(UTF8)
      lines = preprocess_with_lines(yaml.to_s)
      unless lines.empty?
        @handler.start_document([], [], true)
        emit_block(lines, [0], 0)
        @handler.end_document(true)
      end
      @handler.end_stream
      self
    end

    private

    # [[indent, content, lineno], ...] — drops blanks/comments and
    # records the original 0-based line index (for scalar start_line).
    # Block scalars (`key: |`/`>`) are resolved here: their raw,
    # more-indented continuation lines are folded/kept per the
    # indicator and stashed in @block_scalars, leaving a sentinel value
    # the scalar emitter swaps back (so split_key/scalar parsing below
    # never sees the multi-line body).
    def preprocess_with_lines(src)
      raw = src.split("\n", -1)
      out = []
      i = 0
      while i < raw.length
        line = raw[i]
        stripped = line.strip
        if stripped == "---"
          i += 1
          next
        end
        break if stripped == "..."
        if stripped.empty? || stripped.start_with?("#")
          i += 1
          next
        end
        content = RubyrsYAMLParse.strip_trailing_comment(line)
        body = content.strip
        if body.empty?
          i += 1
          next
        end
        # Leading-space count without a per-line regex alloc.
        indent = content.length - content.lstrip.length

        # Block-scalar header detection only matters for lines whose value
        # is `|`/`>` — skip the split_key + regex for the ~99% of lines
        # that contain neither (emit_mapping re-splits the key anyway).
        if (body.include?("|") || body.include?(">")) &&
           (kv = RubyrsYAMLParse.split_key(body)) && kv[1] =~ /\A([|>])([+-]?)(\d*)\s*\z/
          style_ch = $1
          chomp = $2
          block_lines = []
          j = i + 1
          while j < raw.length
            l = raw[j]
            if l.strip.empty?
              block_lines << ""
              j += 1
              next
            end
            break if l[/\A */].length <= indent
            block_lines << l
            j += 1
          end
          value = fold_block_scalar(block_lines, style_ch, chomp)
          token = "@@RUBYRSBLOCK#{@block_scalars.length}@@"
          @block_scalars << value
          out << [indent, "#{kv[0]}: #{token}", i]
          i = j
          next
        end

        out << [indent, body, i]
        i += 1
      end
      out
    end

    # Fold/keep a block scalar's raw continuation lines. `style` is `|`
    # (literal — newlines preserved) or `>` (folded — single newlines
    # between non-blank lines become spaces, blank lines stay newlines).
    # `chomp` is `-` (strip all trailing newlines), `+` (keep them), or
    # "" (clip — exactly one trailing newline). Content is de-indented
    # by the least-indented non-blank line.
    def fold_block_scalar(lines, style, chomp)
      non_blank = lines.reject { |l| l.strip.empty? }
      return chomp == "-" ? "" : "\n" if non_blank.empty?
      base = non_blank.map { |l| l[/\A */].length }.min
      stripped = lines.map { |l| l.strip.empty? ? "" : (l[base..] || "") }
      # Split off trailing blank lines — they only affect chomping.
      trailing = 0
      trailing += 1 while trailing < stripped.length && stripped[-1 - trailing] == ""
      core = stripped[0, stripped.length - trailing]

      body =
        if style == "|"
          core.join("\n")
        else
          folded = +""
          prev_blank = true
          core.each do |l|
            if l.empty?
              folded << "\n"
              prev_blank = true
            else
              folded << " " unless prev_blank
              folded << l
              prev_blank = false
            end
          end
          folded
        end

      case chomp
      when "-" then body
      when "+" then body + ("\n" * (trailing + 1))
      else body.empty? ? "" : body + "\n"
      end
    end

    def emit_block(lines, idx, min_indent, anchor = nil)
      return emit_null(0) if idx[0] >= lines.length
      indent, content, lno = lines[idx[0]]
      return emit_null(lno) if indent < min_indent
      if content.start_with?("- ") || content == "-"
        emit_sequence(lines, idx, indent, anchor)
      elsif RubyrsYAMLParse.split_key(content)
        emit_mapping(lines, idx, indent, anchor)
      else
        idx[0] += 1
        emit_scalar(content, lno, anchor)
      end
    end

    # Split a leading `&anchor` off a value, returning [anchor_or_nil,
    # rest]. `&name` may stand alone (the value is the following block)
    # or precede an inline value (`&name foo`).
    def split_anchor(s)
      if s.start_with?("&")
        m = s.match(/\A&(\S+)\s*(.*)\z/m)
        [m[1], m[2]]
      else
        [nil, s]
      end
    end

    def emit_mapping(lines, idx, indent, anchor = nil)
      @handler.start_mapping(anchor, nil, true, Nodes::Mapping::BLOCK)
      while idx[0] < lines.length
        cur_indent, content, lno = lines[idx[0]]
        break if cur_indent < indent
        break if cur_indent > indent
        kv = RubyrsYAMLParse.split_key(content)
        break if kv.nil?
        key, rest = kv
        idx[0] += 1
        emit_scalar(key, lno)
        # A value may carry a leading `&anchor` (the anchored value is
        # either inline or the following block) — strip it and pass it
        # down so the value node records the anchor.
        val_anchor, rest = split_anchor(rest)
        if rest.empty?
          if idx[0] < lines.length && lines[idx[0]][0] > indent
            emit_block(lines, idx, indent + 1, val_anchor)
          elsif idx[0] < lines.length && lines[idx[0]][0] == indent &&
                (lines[idx[0]][1].start_with?("- ") || lines[idx[0]][1] == "-")
            emit_sequence(lines, idx, indent, val_anchor)
          else
            emit_null(lno)
          end
        else
          emit_scalar(rest, lno, val_anchor)
        end
      end
      @handler.end_mapping
    end

    def emit_sequence(lines, idx, indent, anchor = nil)
      @handler.start_sequence(anchor, nil, true, Nodes::Sequence::BLOCK)
      while idx[0] < lines.length
        cur_indent, content, lno = lines[idx[0]]
        break if cur_indent < indent
        break unless content.start_with?("- ") || content == "-"
        item = content == "-" ? "" : content[2..]
        idx[0] += 1
        if item.strip.empty?
          if idx[0] < lines.length && lines[idx[0]][0] > indent
            emit_block(lines, idx, indent + 1)
          else
            emit_null(lno)
          end
        elsif (kv = RubyrsYAMLParse.split_key(item))
          key, rest = kv
          @handler.start_mapping(nil, nil, true, Nodes::Mapping::BLOCK)
          emit_scalar(key, lno)
          rest.empty? ? emit_null(lno) : emit_scalar(rest, lno)
          item_indent = indent + 2
          while idx[0] < lines.length && lines[idx[0]][0] >= item_indent &&
                (ikv = RubyrsYAMLParse.split_key(lines[idx[0]][1]))
            ik, ir = ikv
            ilno = lines[idx[0]][2]
            idx[0] += 1
            emit_scalar(ik, ilno)
            ir.empty? ? emit_null(ilno) : emit_scalar(ir, ilno)
          end
          @handler.end_mapping
        else
          emit_scalar(item, lno)
        end
      end
      @handler.end_sequence
    end

    # Emit a single scalar node carrying its source line. Quoted forms
    # are unquoted via the shared loader helpers; flow collections
    # recurse into nested sequence/mapping events.
    def emit_scalar(raw, lno, anchor = nil)
      s = raw.to_s.strip
      # Block-scalar sentinel (see preprocess_with_lines): swap back the
      # pre-folded literal/folded value, emitted verbatim (quoted=true so
      # ToRuby returns the string as-is, never tokenizing it). The
      # start_with? guard keeps the regex off the hot path (almost no
      # scalar begins with the sentinel prefix).
      if s.start_with?("@@RUBYRSBLOCK") && s =~ /\A@@RUBYRSBLOCK(\d+)@@\z/
        node = @handler.scalar(@block_scalars[$1.to_i], anchor, nil, false, true, Nodes::Scalar::LITERAL)
        node.start_line = lno if node.respond_to?(:start_line=)
        return node
      end
      # An unquoted leading `*` is an alias reference (resolved against
      # the anchor table by the tree builder / ToRuby visitor).
      if s.start_with?("*")
        node = @handler.alias(s[1..].strip)
        node.start_line = lno if node.respond_to?(:start_line=)
        return node
      end
      if s.start_with?('"')
        node = @handler.scalar(RubyrsYAMLParse.parse_double_quoted(s),
                               anchor, nil, false, true, Nodes::Scalar::DOUBLE_QUOTED)
      elsif s.start_with?("'")
        node = @handler.scalar(RubyrsYAMLParse.parse_single_quoted(s),
                               anchor, nil, false, true, Nodes::Scalar::SINGLE_QUOTED)
      elsif s.start_with?("[")
        return emit_flow_seq(s, lno)
      elsif s.start_with?("{")
        return emit_flow_map(s, lno)
      else
        node = @handler.scalar(s, anchor, nil, true, false, Nodes::Scalar::PLAIN)
      end
      node.start_line = lno if node.respond_to?(:start_line=)
      node
    end

    # A missing mapping/sequence value — an empty plain scalar (psych's
    # null representation). Keeps mapping children paired so RuboCop's
    # `each_slice(2)` lines key/value up correctly.
    def emit_null(lno)
      node = @handler.scalar("", nil, nil, true, false, Nodes::Scalar::PLAIN)
      node.start_line = lno if node.respond_to?(:start_line=)
      node
    end

    def emit_flow_seq(s, lno)
      inner = s[1..-2].to_s.strip
      @handler.start_sequence(nil, nil, true, Nodes::Sequence::FLOW)
      RubyrsYAMLParse.split_flow(inner).each { |e| emit_scalar(e, lno) } unless inner.empty?
      @handler.end_sequence
    end

    def emit_flow_map(s, lno)
      inner = s[1..-2].to_s.strip
      @handler.start_mapping(nil, nil, true, Nodes::Mapping::FLOW)
      unless inner.empty?
        RubyrsYAMLParse.split_flow(inner).each do |pair|
          kv = RubyrsYAMLParse.split_key(pair)
          next unless kv
          emit_scalar(kv[0], lno)
          kv[1].empty? ? emit_null(lno) : emit_scalar(kv[1], lno)
        end
      end
      @handler.end_mapping
    end
  end
end

# ---- node-tree → Ruby materialization --------------------------
#
# The `accept(node_tree)` side RuboCop's config_loader drives directly
# (config_loader.rb:268) instead of calling YAML.load — it reuses the
# Psych::Nodes tree from the duplicate-key check:
#
#   class_loader = YAML::ClassLoader::Restricted.new(%w[Regexp Symbol], [])
#   scanner      = YAML::ScalarScanner.new(class_loader)
#   visitor      = YAML::Visitors::ToRuby.new(scanner, class_loader)
#   visitor.accept(yaml_tree)
#
# Focused port of CRuby psych's ScalarScanner / ClassLoader::Restricted
# / Visitors::ToRuby: enough to materialize .rubocop.yml (mappings,
# sequences, scalars with bool/int/float/symbol/string coercion, the
# `<<` merge key). Out of scope (falls back to string / raises like
# CRuby's restricted loader): arbitrary `!ruby/object:` tags, structs,
# Date/Time scalars, sexagesimal. Anchors/aliases never appear because
# the Parser above doesn't emit them.
module Psych
  class Exception < StandardError; end unless defined?(Psych::Exception)
  class DisallowedClass < Exception
    def initialize(action, klass_name)
      super("Tried to #{action} unspecified class: #{klass_name}")
    end
  end
  class BadAlias < Exception; end

  # Resolves tag class names + symbol coercion. The base allows any
  # constant; Restricted (what RuboCop uses) permits only a named
  # allow-list, raising DisallowedClass otherwise.
  class ClassLoader
    def load(klassname)
      return nil if !klassname || klassname.empty?
      find(klassname)
    end

    def symbolize(sym)
      symbolize_check(sym)
      sym.to_sym
    end

    private

    def symbolize_check(_sym); end

    def find(klassname)
      klassname.to_s.split("::").inject(Object) { |mod, name| mod.const_get(name) }
    end

    class Restricted < ClassLoader
      def initialize(classes, symbols)
        @classes = classes
        @symbols = symbols
        super()
      end

      def symbolize(sym)
        return super if @symbols.empty?
        raise DisallowedClass.new("load", "Symbol") unless @symbols.include?(sym.to_s)
        super
      end

      private

      def find(klassname)
        raise DisallowedClass.new("load", klassname) unless @classes.include?(klassname)
        super
      end
    end
  end

  # Coerces a plain scalar string into a Ruby value (bool / nil / int /
  # float / symbol / string). Faithful to CRuby's tokenize for the
  # cases .rubocop.yml exercises; unhandled exotic forms fall through to
  # the string branch (CRuby's final `else string` too).
  class ScalarScanner
    attr_reader :class_loader

    def initialize(class_loader, strict_integer: false, parse_symbols: true)
      @class_loader = class_loader
      @parse_symbols = parse_symbols
      @symbol_cache = {}
    end

    def tokenize(string)
      return nil if string.empty?
      return @symbol_cache[string] if @symbol_cache.key?(string)
      # Alpha-ish guard (verbatim from CRuby): isolates bool/null/text
      # from numeric forms so "Style/Foo" stays a String and "true"
      # becomes a boolean.
      if string.match?(%r{^[^\d.:-]?[[:alpha:]_\s!@\#$%\^&*(){}<>|/\\~;=]+}) || string.match?(/\n/)
        return string if string.length > 5
        if string.match?(/^[^ytonf~]/i)
          string
        elsif string == "~" || string.match?(/^null$/i)
          nil
        elsif string.match?(/^(yes|true|on)$/i)
          true
        elsif string.match?(/^(no|false|off)$/i)
          false
        else
          string
        end
      elsif string.match?(/^\+?\.inf$/i)
        Float::INFINITY
      elsif string.match?(/^-\.inf$/i)
        -Float::INFINITY
      elsif string.match?(/^\.nan$/i)
        Float::NAN
      elsif @parse_symbols && string.match?(/^:./)
        if string =~ /^:(["'])(.*)\1/
          @symbol_cache[string] = @class_loader.symbolize($2.sub(/^:/, ""))
        else
          @symbol_cache[string] = @class_loader.symbolize(string.sub(/^:/, ""))
        end
      elsif string.match?(/^[-+]?([0-9][0-9_,]*)?\.[0-9]*([eE][-+][0-9]+)?$/)
        return string if string.match?(/\A[-+]?\.\Z/)
        Float(string.delete(",_").gsub(/\.([Ee]|$)/, '\1'))
      elsif string.match?(/\A[-+]?(0b[01_]+|0o[0-7_]+|0x[0-9a-fA-F_]+|[0-9][0-9_]*)\z/)
        Integer(string.delete(",_"))
      else
        string
      end
    end
  end

  module Visitors
    class Visitor
      def accept(target)
        visit(target)
      end

      private

      def visit(target)
        send("visit_#{target.class.name.gsub("::", "_")}", target)
      end
    end

    # Walks a Psych::Nodes tree, producing Ruby objects. Focused subset
    # mirroring CRuby's ToRuby: quoted scalars stay strings, plain
    # scalars run through the ScalarScanner, mappings become Hashes
    # (honouring the `<<` merge key), sequences become Arrays. Symbol /
    # Regexp tags route through the (restricted) class loader.
    class ToRuby < Visitor
      def initialize(scanner, class_loader, symbolize_names: false)
        super()
        @st = {}
        @ss = scanner
        @class_loader = class_loader
        @symbolize_names = symbolize_names
      end

      def visit_Psych_Nodes_Stream(o)
        o.children.map { |c| accept(c) }
      end

      def visit_Psych_Nodes_Document(o)
        accept(o.root)
      end

      def visit_Psych_Nodes_Scalar(o)
        register(o, deserialize(o))
      end

      def visit_Psych_Nodes_Sequence(o)
        seq = register(o, [])
        o.children.each { |c| seq << accept(c) }
        seq
      end

      def visit_Psych_Nodes_Mapping(o)
        revive_hash(register(o, {}), o)
      end

      def visit_Psych_Nodes_Alias(o)
        @st.fetch(o.anchor) { raise BadAlias, "Unknown alias: #{o.anchor}" }
      end

      private

      def register(node, object)
        @st[node.anchor] = object if node.respond_to?(:anchor) && node.anchor
        object
      end

      def deserialize(o)
        return o.value if o.quoted
        return @ss.tokenize(o.value) unless o.tag

        case o.tag
        when /^!(?:str|ruby\/string)(?::(.*))?$/, "tag:yaml.org,2002:str"
          o.value
        when /^!ruby\/sym(?:bol)?:?(.*)?$/, "tag:yaml.org,2002:python/str"
          @class_loader.symbolize(o.value)
        when /^!ruby\/regexp/
          # `!ruby/regexp /pattern/opts` — split off the trailing option
          # letters, map to Regexp flags. Restricted permits Regexp.
          @class_loader.load("Regexp")
          source = o.value
          if source =~ %r{\A/(.*)/([mixn]*)\z}m
            body, opts = $1, $2
            flags = 0
            flags |= Regexp::MULTILINE if opts.include?("m")
            flags |= Regexp::IGNORECASE if opts.include?("i")
            flags |= Regexp::EXTENDED if opts.include?("x")
            Regexp.new(body, flags)
          else
            Regexp.new(source)
          end
        else
          @ss.tokenize(o.value)
        end
      end

      def revive_hash(hash, o)
        o.children.each_slice(2) do |k, v|
          key = accept(k)
          val = accept(v)

          if key == "<<" && k.tag != "tag:yaml.org,2002:str"
            case v
            when Nodes::Mapping
              begin
                hash.merge!(val)
              rescue TypeError
                hash[key] = val
              end
            when Nodes::Sequence
              begin
                val.reverse_each { |value| hash.merge!(value) }
              rescue TypeError
                hash[key] = val
              end
            else
              hash[key] = val
            end
          else
            key = key.to_sym if @symbolize_names && key.is_a?(String)
            hash[key] = val
          end
        end
        hash
      end
    end
  end
end

# RuboCop's config_loader references the materialization classes under
# the YAML namespace (`YAML::ClassLoader::Restricted`,
# `YAML::ScalarScanner`, `YAML::Visitors::ToRuby`). YAML and Psych are
# the same module object, but rubyrs keys constants by their textual
# path — so EACH qualified name needs its own binding, including the
# nested ones (`YAML::ClassLoader` resolving is not enough;
# `YAML::ClassLoader::Restricted` is a distinct textual key). Same
# pattern yaml.rb uses for `Psych::Exception = YAML::Exception`.
module YAML
  ClassLoader = Psych::ClassLoader unless defined?(YAML::ClassLoader)
  ScalarScanner = Psych::ScalarScanner unless defined?(YAML::ScalarScanner)
  Visitors = Psych::Visitors unless defined?(YAML::Visitors)
end
unless defined?(YAML::ClassLoader::Restricted)
  YAML::ClassLoader::Restricted = Psych::ClassLoader::Restricted
end
unless defined?(YAML::Visitors::Visitor)
  YAML::Visitors::Visitor = Psych::Visitors::Visitor
end
unless defined?(YAML::Visitors::ToRuby)
  YAML::Visitors::ToRuby = Psych::Visitors::ToRuby
end
