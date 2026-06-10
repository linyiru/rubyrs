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
      # lexer Class → { [state_name, rule_index] => original rule Proc }
      # for the v2 session protocol. Kept on the Ruby side so the procs
      # stay GC-rooted through this registry.
      @callbacks = {}

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
        attr_reader :rules, :procs
        def initialize(upgrade, trust_blocks)
          @rules = []
          @procs = {}
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
              @procs[@rules.length] = blk
              @rules << { "kind" => "callback", "re" => re.source, "opts" => re.options }
              return
            end
            ctx = TraceCtx.new
            begin
              ctx.instance_exec(:__stub__, &blk)
              @rules << { "kind" => "actions", "re" => re.source,
                          "opts" => re.options, "actions" => ctx.actions }
            rescue StandardError
              @procs[@rules.length] = blk
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
        callbacks = {}
        upgrade = WORDLIST_BUILDERS[lexer_class.name]
        trust = TRACE_ALLOWLIST[lexer_class.name] ? true : false
        lexer_class.state_definitions.each do |name, dsl|
          defn = dsl.instance_variable_get(:@defn)
          return nil unless defn
          rec = Recorder.new(upgrade, trust)
          rec.instance_eval(&defn)
          states[name.to_s] = rec.rules
          rec.procs.each { |idx, blk| callbacks[[name.to_s, idx]] = blk }
        end
        json = JSON.generate({ "states" => states, "shortnames" => shortnames })
        id = __rubyrs_rouge_native_table(json)
        @callbacks[lexer_class] = callbacks if id
        id
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

      # ---- v2 session protocol: the engine pauses on a callback rule;
      # we run the ORIGINAL rouge block on a real lexer instance with the
      # DSL verbs intercepted into an ops buffer, replay the ops into the
      # engine, and resume. Any surprise (unknown proc, exception inside
      # the block, delegate, apply failure) aborts the session — the
      # caller falls back to pure rouge for that call.
      # Session protocol (JSON-free): `lex_run` returns a tagged string —
      # "D"+html (done) / "C{rule}:{state}" (paused on a callback rule) /
      # "E" (error). On a pause we execute the ORIGINAL rouge block on a
      # real lexer instance; the patched DSL verbs stream effects through
      # the op_* host fns, match groups are fetched lazily via group(),
      # and lex_resume applies + continues.
      def self.lex_html_session(lexer_class, tid, source)
        sid = __rubyrs_rouge_native_lex_start(tid, source)
        procs = @callbacks[lexer_class] || {}
        inst = nil
        loop do
          reply = __rubyrs_rouge_native_lex_run(sid)
          tag = reply[0]
          if tag == "D"
            return reply[1..-1]
          elsif tag == "C"
            body = reply[1..-1]
            colon = body.index(":")
            rule_idx = body[0...colon].to_i
            state = body[(colon + 1)..-1]
            blk = procs[[state, rule_idx]]
            unless blk
              __rubyrs_rouge_native_lex_abort(sid)
              return nil
            end
            inst ||= begin
              l = lexer_class.new
              # Run start_procs so helper state the blocks rely on
              # (e.g. python's string register) exists; the Ruby-side
              # @stack it also sets is unused during native lexing.
              begin
                l.send(:reset!)
              rescue StandardError
                nil
              end
              l
            end
            unless run_bridged(inst, blk, sid) && __rubyrs_rouge_native_lex_resume(sid)
              __rubyrs_rouge_native_lex_abort(sid)
              return nil
            end
          else
            # "E" — the host already freed the session.
            return nil
          end
        end
      rescue StandardError
        __rubyrs_rouge_native_lex_abort(sid) if sid
        nil
      end

      # Execute a rule block on `inst` with the DSL verbs streaming to
      # the host (see the RegexLexer patches below). Returns true, or
      # false/nil when the block did anything we can't replay.
      def self.run_bridged(inst, blk, sid)
        m = MatchStub.new(sid)
        stack_before = inst.instance_variable_get(:@stack)
        depth_before = stack_before ? stack_before.length : nil
        inst.instance_variable_set(:@__carmine_sid, sid)
        begin
          inst.instance_exec(m, &blk)
          # If the block poked the PURE state stack directly (bypassing
          # the patched verbs), its effect would be silently lost on the
          # native side — abort instead.
          stack_after = inst.instance_variable_get(:@stack)
          depth_after = stack_after ? stack_after.length : nil
          depth_before == depth_after
        rescue StandardError
          false
        ensure
          inst.instance_variable_set(:@__carmine_sid, nil)
        end
      end

      # What rule blocks receive as `m` — group access by index, like the
      # StringScanner rouge passes ([0] whole match, [i] capture i).
      # Groups are fetched lazily from the paused host session.
      class MatchStub
        def initialize(sid)
          @sid = sid
        end
        def [](i)
          __rubyrs_rouge_native_group(@sid, i)
        end
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

      # ---- v2 bridge: while a session callback is being executed
      # (@__carmine_sid set by CarmineNative.run_bridged), the DSL verbs
      # stream their effects to the paused host session instead of
      # touching the pure-Ruby lexer state. Outside a bridge call they
      # behave exactly as before.
      alias_method :__carmine_pure_token, :token
      def token(tok, *rest)
        if (sid = @__carmine_sid)
          val = rest.empty? ? __rubyrs_rouge_native_group(sid, 0) : rest[0]
          __rubyrs_rouge_native_op_token(sid, tok.qualname, val.to_s) unless val.nil?
          nil
        else
          __carmine_pure_token(tok, *rest)
        end
      end

      alias_method :__carmine_pure_groups, :groups
      def groups(*tokens)
        if (sid = @__carmine_sid)
          tokens.each_with_index do |tok, i|
            val = __rubyrs_rouge_native_group(sid, i + 1)
            __rubyrs_rouge_native_op_token(sid, tok.qualname, val) unless val.nil?
          end
          nil
        else
          __carmine_pure_groups(*tokens)
        end
      end

      alias_method :__carmine_pure_push, :push
      def push(state_name = nil, &b)
        if (sid = @__carmine_sid)
          # Block-form push (anonymous state) can't be replayed — bail
          # so the session aborts and the call falls back.
          raise "carmine: block push unsupported" if b
          __rubyrs_rouge_native_op_push(sid, state_name&.to_s)
          nil
        else
          __carmine_pure_push(state_name, &b)
        end
      end

      alias_method :__carmine_pure_pop!, :pop!
      def pop!(times = 1)
        if (sid = @__carmine_sid)
          __rubyrs_rouge_native_op_pop(sid, times)
          nil
        else
          __carmine_pure_pop!(times)
        end
      end

      alias_method :__carmine_pure_goto, :goto
      def goto(state_name)
        if (sid = @__carmine_sid)
          __rubyrs_rouge_native_op_goto(sid, state_name.to_s)
          nil
        else
          __carmine_pure_goto(state_name)
        end
      end

      # Verbs that can't be answered/replayed during a bridged callback
      # (they depend on the PURE lexer's state stack, which the native
      # engine owns) raise — run_bridged catches and the session aborts
      # to the pure fallback. Silent wrong answers are the enemy here.
      [:delegate, :in_state?, :state?, :recurse].each do |verb|
        next unless method_defined?(verb) || private_method_defined?(verb)
        pure = "__carmine_pure_#{verb.to_s.delete('?')}"
        alias_method pure, verb
        define_method(verb) do |*a, &b2|
          raise "carmine: #{verb} unsupported in bridged callback" if @__carmine_sid
          send(pure, *a, &b2)
        end
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
            # Fast path: no callback rule hit — one native shot.
            out = __rubyrs_rouge_native_lex_html(tokens.table_id, tokens.source)
            # v2: a callback rule fired — run the session protocol (the
            # engine pauses, we execute the original block, it resumes).
            out ||= Rouge::CarmineNative.lex_html_session(
              tokens.lexer_class, tokens.table_id, tokens.source
            )
            if out
              b.call(out)
              return
            end
            # Session aborted (unreplayable block) — pure fallback.
            tokens = tokens.lexer_class.new.__carmine_pure_lex(tokens.source)
          end
          __carmine_pure_stream(tokens, &b)
        end
      end
    end
  end
end
