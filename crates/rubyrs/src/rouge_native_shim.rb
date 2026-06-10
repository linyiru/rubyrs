# _rouge_native shim — injected by rubyrs right after `require "rouge"`
# completes (see vm/kernel.rs). Routes supported lexers through the
# carmine native engine (byte-identical HTML); everything unsupported
# falls back to the pure-Ruby gem:
#   - per LEXER: table extraction/compilation declines → cached false,
#     that lexer stays pure-Ruby forever;
#   - per CALL: a match-dependent rule (kind "callback") fires mid-lex →
#     the host returns nil and this one call re-runs through pure rouge;
#   - per CONSUMER: anything that iterates the lex result instead of
#     going through the patched HTML formatter gets real tokens via
#     NativeLexProxy#each's pure-Ruby fallback.
if defined?(__rubyrs_rouge_native_table) && defined?(Rouge::RegexLexer)
  require "json"

  module Rouge
    module CarmineNative
      @tables = {}

      # Trace context for rule blocks: records static DSL call chains
      # (groups/token/push/pop!/goto with constant args); anything
      # match-dependent raises and the rule is marked "callback".
      class TraceCtx
        attr_reader :actions
        class Bail < StandardError; end
        def initialize
          @actions = []
        end
        def groups(*toks)
          @actions << ["groups", toks.map { |t| t.qualname }]
        end
        def token(tok, val = :__default__)
          raise Bail unless val == :__default__
          @actions << ["token", tok.qualname]
        end
        def push(st = :__self__)
          @actions << ["push", st.to_s]
        end
        def pop!(n = 1)
          @actions << ["pop", n]
        end
        def goto(st)
          @actions << ["goto", st.to_s]
        end
        def method_missing(*args)
          raise Bail
        end
        def respond_to_missing?(*args)
          true
        end
      end

      # SOUNDNESS: trace-once records a LINEAR path through a rule
      # block. A block whose conditional reads instance state (`if
      # @heredoc ...`) silently takes the nil branch on the stub and
      # would be mis-compiled — so traced actions are trusted ONLY for
      # lexers on this verified allowlist (byte-identical against the
      # real gem, all action rules exercised). Every other lexer's
      # block rules become "callback": the declarative rules still run
      # natively, and any call that hits a block rule falls back to
      # pure rouge for that call.
      TRACE_ALLOWLIST = {
        "Rouge::Lexers::Python" => true,
      }

      # The universal identifier-classification idiom (`if
      # keywords.include?(m[0]) ...`) is match-dependent, but its word
      # sets are class-level DATA — upgrade those rules to native
      # wordlist kind per lexer.
      WORDLIST_BUILDERS = {
        "Rouge::Lexers::Python" => lambda do |re_source|
          if re_source.include?('(?<!\.)')
            { "kind" => "wordlist",
              "sets" => [
                ["Keyword", Rouge::Lexers::Python.keywords],
                ["Name.Builtin", Rouge::Lexers::Python.exceptions],
                ["Name.Builtin", Rouge::Lexers::Python.builtins],
                ["Name.Builtin.Pseudo", Rouge::Lexers::Python.builtins_pseudo],
              ],
              "default" => "Name" }
          end
        end,
      }

      class Recorder
        attr_reader :rules
        def initialize(upgrade, trust_blocks)
          @rules = []
          @upgrade = upgrade
          @trust_blocks = trust_blocks
        end
        def rule(re, tok = nil, next_state = nil, &blk)
          if blk
            if @upgrade && (spec = @upgrade.call(re.source))
              @rules << spec.merge("re" => re.source, "opts" => re.options)
              return
            end
            unless @trust_blocks
              @rules << { "kind" => "callback", "re" => re.source, "opts" => re.options }
              return
            end
            ctx = TraceCtx.new
            begin
              ctx.instance_exec(:__stub__, &blk)
              @rules << { "kind" => "actions", "re" => re.source,
                          "opts" => re.options, "actions" => ctx.actions }
            rescue StandardError
              @rules << { "kind" => "callback", "re" => re.source, "opts" => re.options }
            end
          else
            ns = case next_state
                 when nil then nil
                 when Array then next_state.map { |s| s.to_s }
                 else next_state.to_s
                 end
            @rules << { "kind" => "tok", "re" => re.source, "opts" => re.options,
                        "tok" => tok.qualname, "next" => ns }
          end
        end
        def mixin(name)
          @rules << { "kind" => "mixin", "state" => name.to_s }
        end
      end

      def self.shortnames
        @shortnames ||= begin
          sn = {}
          Rouge::Token.each_token { |t| sn[t.qualname] = t.shortname }
          sn
        end
      end

      def self.build_table(lexer_class)
        states = {}
        upgrade = WORDLIST_BUILDERS[lexer_class.name]
        trust = TRACE_ALLOWLIST[lexer_class.name] ? true : false
        lexer_class.state_definitions.each do |name, dsl|
          defn = dsl.instance_variable_get(:@defn)
          return nil unless defn
          rec = Recorder.new(upgrade, trust)
          rec.instance_eval(&defn)
          states[name.to_s] = rec.rules
        end
        json = JSON.generate({ "states" => states, "shortnames" => shortnames })
        __rubyrs_rouge_native_table(json)
      end

      # Integer table id, or false when this lexer is declined (table
      # extraction raised, or the engine couldn't compile it).
      def self.table_id(lexer_class)
        cached = @tables[lexer_class]
        return cached unless cached.nil?
        id = begin
          build_table(lexer_class)
        rescue StandardError
          nil
        end
        @tables[lexer_class] = id ? id : false
      end
    end

    # Lazy stand-in returned by the patched RegexLexer#lex. The patched
    # HTML formatter short-circuits it through the native engine; any
    # OTHER consumer iterating it gets real tokens from a pure-Ruby lex.
    class CarmineNativeLex
      include Enumerable
      attr_reader :lexer_class, :source, :table_id
      def initialize(lexer_class, source, table_id)
        @lexer_class = lexer_class
        @source = source
        @table_id = table_id
      end
      def each(&b)
        @lexer_class.new.__carmine_pure_lex(@source).each(&b)
      end
    end

    class RegexLexer
      alias_method :__carmine_pure_lex, :lex
      def lex(source, opts = nil, &b)
        if b.nil? && opts.nil?
          tid = CarmineNative.table_id(self.class)
          return CarmineNativeLex.new(self.class, source, tid) if tid
        end
        __carmine_pure_lex(source, opts, &b)
      end
    end

    # Base Formatter#format wraps tokens in
    # `enum_for(:filter_escapes, tokens)` (escape-disabled default),
    # which would re-box the proxy and push it onto the pure-Ruby
    # `#each` fallback. Bypass the wrap for proxies: accepted tables
    # never emit `Escape` (the host declines those), so filter_escapes
    # is an identity for them and calling `stream` directly is
    # byte-identical.
    class Formatter
      alias_method :__carmine_pure_format, :format
      def format(tokens, &b)
        if tokens.is_a?(Rouge::CarmineNativeLex)
          return stream(tokens, &b) if b
          out = +""
          stream(tokens) { |piece| out << piece }
          return out
        end
        __carmine_pure_format(tokens, &b)
      end
    end

    module Formatters
      class HTML
        alias_method :__carmine_pure_stream, :stream
        def stream(tokens, &b)
          if tokens.is_a?(Rouge::CarmineNativeLex)
            out = __rubyrs_rouge_native_lex_html(tokens.table_id, tokens.source)
            if out
              b.call(out)
              return
            end
            # A callback rule fired mid-lex — fall back for this call.
            tokens = tokens.lexer_class.new.__carmine_pure_lex(tokens.source)
          end
          __carmine_pure_stream(tokens, &b)
        end
      end
    end
  end
end
