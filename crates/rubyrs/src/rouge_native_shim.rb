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
# RUBYRS_NATIVE_STATS=1: count native-accelerator hits vs declines
# (the 2.8x new-corpus regression hunt — see RUBYRS_REGEX_STATS for
# the pattern). Zero cost when the env var is unset (one nil check
# per counter site).
if ENV["RUBYRS_NATIVE_STATS"] && !$__rubyrs_native_stats
  $__rubyrs_native_stats = Hash.new(0)
  at_exit do
    $stderr.puts "[native-stats] " +
      $__rubyrs_native_stats.sort.map { |k, v| "#{k}=#{v}" }.join(" ")
  end
end

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
            # Track C: AST→IR compilation. The host parses the lexer
            # FILE (via the proc's source_location), whitelists the
            # block's AST and returns carmine IR ops — or nil, and the
            # rule falls through to trace/callback exactly as before.
            if defined?(__rubyrs_rouge_native_compile_proc) &&
               (ir = Recorder.compile_block_ir(blk))
              @rules << { "kind" => "ir", "re" => re.source,
                          "opts" => re.options, "ops" => ir }
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

        # AST→IR for one rule block: host compiles, then token
        # CONSTANT PATHS resolve to qualnames through the live token
        # tree (rouge constants alias — `Str` IS `Literal::String`,
        # so paths cannot be resolved offline). Any failure → nil and the
        # caller keeps the callback path.
        def self.compile_block_ir(blk)
          # NOTE: direct call, not respond_to? — builtin dispatch arms
          # aren't visible to respond_to? (VM gap); the rescue below
          # covers runtimes without the method.
          loc = begin
            blk.source_location
          rescue NoMethodError
            nil
          end
          return nil unless loc && loc[0] && loc[1]
          json = __rubyrs_rouge_native_compile_proc(loc[0], loc[1])
          return nil unless json
          ops = JSON.parse(json)
          resolve_ir_tokens!(ops) ? ops : nil
        rescue StandardError
          nil
        end

        def self.resolve_ir_tokens!(ops)
          ops.each do |op|
            case op[0]
            when "token"
              q = ir_qualname(op[1])
              return false unless q
              op[1] = q
            when "groups"
              op[1].map! do |n|
                q = ir_qualname(n)
                return false unless q
                q
              end
            when "if"
              return false unless resolve_ir_tokens!(op[2])
              return false if op[3] && !resolve_ir_tokens!(op[3])
            end
          end
          true
        end

        def self.ir_qualname(path)
          t = path.split("::").inject(Rouge::Token::Tokens) do |mod_, c|
            mod_.const_get(c)
          end
          t.respond_to?(:qualname) ? t.qualname : nil
        rescue StandardError
          nil
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
          if $__rubyrs_native_stats
            key = tid ? :rg_native : :"rg_decline_#{self.class.tag rescue self.class}"
            $__rubyrs_native_stats[key] += 1
          end
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

# ---- lazy lexer loading (lexer-gate) -------------------------------
# When the kramdown accelerator raised the host lexer-gate BEFORE
# `require "rouge"` (it does so only after matching the on-disk rouge
# version against its embedded static tables), rouge.rb's eager
# `Rouge.load_lexers` walk over all 227 lexer files was skipped by the
# host. Install demand loading so the registry still resolves every
# lexer: Lexer.find misses load the one file (alias-mapped), and
# registry-enumerating APIs load everything first. Without the gate
# flag this section is inert and rouge behaves exactly as upstream.
if defined?(__rubyrs_rouge_native_table) && defined?(Rouge::RegexLexer) &&
   $__rubyrs_rouge_lexer_gate
  module Rouge
    module CarmineNative
      # tag/alias → lexer FILE BASENAME, generated from rouge 4.7.0's
      # lexers/*.rb `tag`/`aliases` declarations (only non-identity
      # entries; identical names resolve directly). Regenerate together
      # with the static tables when bumping rouge.
      LEXER_ALIAS_FILE = {
    "Containerfile" => "docker", "Dockerfile" => "docker",
    "HAML" => "haml", "Isabelle" => "isabelle", "LaTeX" => "tex",
    "OCL" => "ocl", "R" => "r", "S" => "r", "TeX" => "tex",
    "abl" => "openedge", "apib" => "apiblueprint",
    "applescript" => "apple_script", "as" => "actionscript",
    "as3" => "actionscript", "aug" => "augeas", "bash" => "shell",
    "bat" => "batchfile", "batch" => "batchfile", "behat" => "gherkin",
    "bf" => "brainfuck", "bib" => "bibtex", "brs" => "brightscript",
    "bs" => "brightscript", "bsdmake" => "make", "c#" => "csharp",
    "c++" => "cpp", "cfc" => "cfscript", "cl" => "common_lisp",
    "clj" => "clojure", "cljs" => "clojure", "cmm" => "ghc_cmm",
    "coffee" => "coffeescript", "coffee-script" => "coffeescript",
    "common-lisp" => "common_lisp", "config" => "conf",
    "configuration" => "conf", "containerfile" => "docker",
    "cr" => "crystal", "cs" => "csharp", "cucumber" => "gherkin",
    "django" => "jinja", "dlang" => "d", "dockerfile" => "docker",
    "dosbatch" => "batchfile", "e-mail" => "email",
    "elisp" => "common_lisp", "emacs-lisp" => "common_lisp",
    "eml" => "email", "eps" => "postscript", "erl" => "erlang",
    "eruby" => "erb", "esc" => "escape", "ex" => "viml", "exs" => "elixir",
    "fea" => "opentype_feature_file", "ff" => "freefem", "ftl" => "fluent",
    "gd" => "gdscript", "ghc-cmm" => "ghc_cmm", "ghc-core" => "ghc_core",
    "gnumake" => "make", "golang" => "go", "graphviz" => "dot",
    "hbs" => "handlebars", "heex" => "eex", "hh" => "hack",
    "hs" => "haskell", "hx" => "haxe", "hy" => "hylang", "idr" => "idris",
    "isa" => "isabelle", "jdn" => "janet", "jl" => "julia",
    "js" => "javascript", "json-doc" => "json_doc", "jsonc" => "json_doc",
    "kdb+" => "q", "ksh" => "shell", "lassoscript" => "lasso",
    "latex" => "tex", "leex" => "eex", "lhaskell" => "literate_haskell",
    "lhs" => "literate_haskell", "lisp" => "common_lisp",
    "litcoffee" => "literate_coffeescript",
    "lithaskell" => "literate_haskell", "ls" => "livescript",
    "m" => "matlab", "makefile" => "make", "md" => "markdown",
    "mf" => "make", "microsoftshell" => "powershell", "mkd" => "markdown",
    "ml" => "sml", "moon" => "moonscript", "msshell" => "powershell",
    "mustache" => "handlebars", "nes" => "nesasm", "nextflow" => "groovy",
    "nf" => "groovy", "nimrod" => "nim", "nixos" => "nix",
    "obj-c" => "objective_c", "obj-cpp" => "objective_cpp",
    "obj_c" => "objective_c", "obj_cpp" => "objective_cpp",
    "objc" => "objective_c", "objcpp" => "objective_cpp",
    "objective-c" => "objective_c", "objectivec" => "objective_c",
    "objectivecpp" => "objective_cpp",
    "opentype" => "opentype_feature_file",
    "opentypefeature" => "opentype_feature_file", "patch" => "diff",
    "php3" => "php", "php4" => "php", "php5" => "php", "pl" => "perl",
    "plaintext" => "plain_text", "posh" => "powershell",
    "postscr" => "postscript", "pp" => "puppet", "proto" => "protobuf",
    "pry" => "irb", "ps" => "postscript", "py" => "python",
    "pyrex" => "cython", "pyx" => "cython", "rb" => "ruby",
    "react" => "jsx", "realbasic" => "xojo", "rhtml" => "erb",
    "robot" => "robot_framework", "robot-framework" => "robot_framework",
    "rs" => "rust", "s" => "r", "sh" => "shell",
    "shell-session" => "console", "shell_session" => "console",
    "squeak" => "smalltalk", "st" => "smalltalk", "terminal" => "console",
    "text" => "plain_text", "tf" => "terraform", "ts" => "typescript",
    "udiff" => "diff", "unit-file" => "systemd",
    "varnishconf" => "varnish", "vcl" => "varnish", "vim" => "viml",
    "vimscript" => "viml", "visualbasic" => "vb", "vuejs" => "vue",
    "winbatch" => "batchfile", "wl" => "mathematica",
    "wolfram" => "mathematica", "yml" => "yaml", "zir" => "zig",
    "zsh" => "shell"
  }.freeze

      def self.demand_load_lexer(name)
        dir = ::Rouge::Lexers::BASE_DIR
        file = "#{LEXER_ALIAS_FILE[name] || name}.rb"
        return false unless File.exist?(File.join(dir, file))
        __rubyrs_rouge_native_lexer_gate(false)
        begin
          ::Rouge::Lexers.load_lexer(file)
        ensure
          __rubyrs_rouge_native_lexer_gate(true)
        end
        true
      end

      # Restore the eager world: lower the gate for good and run the
      # original full walk (load_lexer dedupes already-loaded files).
      def self.load_all_lexers
        return if @all_lexers_loaded
        @all_lexers_loaded = true
        $__rubyrs_rouge_lexer_gate = false
        __rubyrs_rouge_native_lexer_gate(false)
        ::Rouge.load_lexers
        nil
      end
    end

    class Lexer
      class << self
        # NOTE: alias_method, NOT `method(:find)` capture — rubyrs
        # Method objects re-dispatch by name at call time, so a
        # captured `find` would resolve to THIS redefinition and
        # recurse.
        alias_method :__carmine_orig_find, :find
        def find(name)
          found = __carmine_orig_find(name)
          if !found && $__rubyrs_rouge_lexer_gate && !name.nil?
            s = name.to_s
            ::Rouge::CarmineNative.demand_load_lexer(s)
            found = __carmine_orig_find(s)
            unless found
              # Unknown alias / tag-vs-filename mismatch: load the
              # world so a real miss means the same thing it means
              # upstream.
              ::Rouge::CarmineNative.load_all_lexers
              found = __carmine_orig_find(s)
            end
          end
          found
        end

        # Registry-enumerating APIs see the full set, exactly as
        # eager loading would.
        alias_method :__carmine_orig_all, :all
        def all
          ::Rouge::CarmineNative.load_all_lexers if $__rubyrs_rouge_lexer_gate
          __carmine_orig_all
        end
      end
    end
  end
end
