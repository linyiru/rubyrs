# _kramdown_native shim — injected by rubyrs right after
# `require "kramdown-parser-gfm"` completes (see vm/kernel.rs). That is
# the moment Jekyll's KramdownParser#load_dependencies runs, so
# Kramdown::JekyllDocument is already defined. Routes documents whose
# options exactly match Jekyll's kramdown defaults through the rostdown
# native renderer (byte-identical HTML); everything else falls back to
# the pure-Ruby gem:
#   - per DOCUMENT: rostdown declines any construct outside its subset
#     (host returns nil) → the original Kramdown parse runs;
#   - per OPTIONS: any key/value outside the verified Jekyll-default
#     whitelist → ineligible, pure Ruby;
#   - code blocks are highlighted by THIS side via the same
#     Rouge-formatter path kramdown's rouge plugin uses (accelerated by
#     _rouge_native when active), so highlighting stays byte-identical
#     by construction.
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

# Runs whenever the `_kramdown_native` host fns are present (the shim is
# injected after `require "kramdown-parser-gfm"`). It patches whichever
# Kramdown::Document subclass the active framework defines — Jekyll's
# JekyllDocument and/or Bridgetown's BridgetownDocument — installing each
# only once (idempotent), so re-injection at a later require point picks
# up a class that wasn't defined yet on an earlier pass.
if defined?(__rubyrs_kd_scan) && defined?(::Kramdown)
  module Kramdown
    unless defined?(RostdownNative)
    module RostdownNative
      # The exact options hash Jekyll 4.4 passes to JekyllDocument.new
      # (probed from a real build; KramdownParser#setup adds the
      # syntax_highlighter keys). Values that affect rendering must
      # match exactly; IGNORED keys can't influence rostdown's subset
      # (toc/footnotes/math are declined constructs, warnings are
      # empty, coderay is dead legacy config).
      REQUIRED = {
        "input" => "GFM",
        "hard_wrap" => false,
        "auto_ids" => true,
        "guess_lang" => true,
        "entity_output" => "as_char",
        "smart_quotes" => "lsquo,rsquo,ldquo,rdquo",
        "syntax_highlighter" => "rouge",
      }.freeze
      HL_OPTS = { "default_lang" => "plaintext", "guess_lang" => true }.freeze
      # The same options after kramdown's highlighter setup
      # normalises them (symbol keys) — see eligible?.
      HL_OPTS_SYM = { default_lang: "plaintext", guess_lang: true }.freeze
      # Bridgetown's KramdownParser only sets `guess_lang` (no
      # `default_lang`). For fenced code blocks `default_lang` changes the
      # no-language path, so `try_render` declines a Bridgetown-shaped
      # config when the source actually has a fence; prose (the common
      # case) renders identically. Both key spellings, as with HL_OPTS.
      BT_HL_OPTS = { "guess_lang" => true }.freeze
      BT_HL_OPTS_SYM = { guess_lang: true }.freeze
      # `include_extraction_tags` is a Bridgetown-only kramdown key (its
      # serbea/markdown extraction pass); false is the render-neutral
      # default. `mark_highlighting` is Bridgetown's `==mark==`/`::ins::`
      # extension — rostdown has no such construct, but for a source
      # WITHOUT those delimiters the output is byte-identical either way,
      # so it's accepted here and guarded per-document in `try_render`.
      IGNORED = [
        "toc_levels", "footnote_nr", "show_warnings", "coderay",
        "include_extraction_tags", "mark_highlighting"
      ].freeze

      def self.eligible?(options)
        return false unless options.is_a?(Hash)
        options.each do |k, v|
          next if IGNORED.include?(k)
          if k == "syntax_highlighter_opts"
            # Accept BOTH key spellings: Jekyll passes string keys,
            # but the first document that takes the pure-kramdown
            # path (any rostdown decline) lets kramdown's
            # syntax-highlighter setup write a SYMBOL-keyed
            # normalisation of this hash back into the converter's
            # shared @config — after which a string-only comparison
            # declined every subsequent document forever (the
            # re-render "transition round" pathology: one decline
            # cascaded into 358 pure-kramdown conversions).
            # Bridgetown's lighter `{guess_lang:}` shape is accepted too;
            # the code-fence guard in `try_render` covers its missing
            # `default_lang`.
            return false unless [HL_OPTS, HL_OPTS_SYM, BT_HL_OPTS, BT_HL_OPTS_SYM].include?(v)
            next
          end
          return false unless REQUIRED.key?(k) && REQUIRED[k] == v
        end
        REQUIRED.each_key do |k|
          return false unless options.key?(k)
        end
        true
      end

      # Render `source` natively iff the options are eligible AND nothing
      # in the source needs a construct rostdown can't reproduce. The one
      # framework-specific guard: Bridgetown's `mark_highlighting` turns
      # `==x==` / `::x::` into `<mark>` / `<ins>`; rostdown has no such
      # rule, so a source containing those delimiters must fall back to
      # the pure-Ruby parse (a plain substring scan — cheaper than a parse
      # and almost always false for prose). Returns the HTML or nil
      # (caller runs the original Ruby parse on nil).
      def self.try_render(source, options, flavor)
        return nil unless eligible?(options)
        if flavor == "bridgetown"
          # rostdown has no `==mark==`/`::ins::`; for a source without
          # those delimiters the output is identical either way.
          if (options["mark_highlighting"] || options[:mark_highlighting]) &&
             (source.include?("==") || source.include?("::"))
            return nil
          end
          # A NO-LANGUAGE fence (` ``` ` with no info string) is the one
          # code construct that diverges: Bridgetown (no `default_lang`)
          # wraps it in `<div class="highlighter-rouge">` and rouge GUESSES
          # the language (`guess_lang: true`), which rostdown can't
          # reproduce. Inline code spans and language-tagged fences (the
          # common cases) DO render natively. `has_bare_opening_fence?`
          # tracks open/close so a closing fence isn't mistaken for a
          # bare opener.
          return nil if has_bare_opening_fence?(source)
        end
        render(source, flavor)
      end

      # True if `source` opens a fenced code block with no language/info
      # string. Walks fences in order so the bare CLOSING fence of a
      # language-tagged block (` ```ruby ` … ` ``` `) is not counted.
      def self.has_bare_opening_fence?(source)
        in_fence = false
        source.each_line do |line|
          # chomp: each_line keeps the trailing "\n", which would break the
          # `\z` anchor — an opening `` ```ruby `` then wouldn't match while
          # the bare closing `` ``` `` (last line, no newline) would, and a
          # closing fence would be misread as a bare opener.
          next unless (m = line.chomp.match(/\A[ ]{0,3}(?:`{3,}|~{3,})(.*)\z/))
          if in_fence
            in_fence = false
          else
            in_fence = true
            return true if m[1].strip.empty?
          end
        end
        false
      end

      # Patch `klass#initialize`/`#to_html` (a Kramdown::Document subclass —
      # Jekyll's JekyllDocument or Bridgetown's BridgetownDocument) to try
      # the native renderer first. Idempotent; both subclasses share the
      # wrapper since the only contract Jekyll/Bridgetown rely on after
      # construction is `#to_html` (+ `#warnings`).
      # Minimal stand-in for `document.root` on the native path (we skip
      # building the kramdown AST). Jekyll only reads `#to_html`/`#warnings`,
      # but Bridgetown's KramdownParser#convert reads
      # `document.root.options[:extractions]` — so `root.options` must be a
      # Hash (extractions ⇒ nil). Shared + frozen; read-only.
      class StubRoot
        def options
          {}
        end
      end
      STUB_ROOT = StubRoot.new

      def self.install(klass, flavor)
        return if klass.method_defined?(:__rostdown_orig_initialize)
        klass.send(:alias_method, :__rostdown_orig_initialize, :initialize)
        # `*rest` (not `options = {}`): a define_method block's optional
        # positional doesn't relax the arity check the way a `def` default
        # does, so `Doc.new(src, opts)` would raise "wrong number of
        # arguments (given 2, expected 1)". Splat accepts both arities.
        klass.send(:define_method, :initialize) do |source, *rest|
          options = rest.first || {}
          $__rubyrs_native_stats[:kd_total] += 1 if $__rubyrs_native_stats
          @__rostdown_html = nil
          html = ::Kramdown::RostdownNative.try_render(source, options, flavor)
          if $__rubyrs_native_stats
            if ::Kramdown::RostdownNative.eligible?(options)
              $__rubyrs_native_stats[html ? :kd_native : :kd_decline] += 1
            else
              $__rubyrs_native_stats[:kd_ineligible] += 1
            end
          end
          if html
            # Skip the Ruby parse entirely; only #to_html / #warnings run after.
            @__rostdown_html = html
            @options = options
            @warnings = []
            @root = ::Kramdown::RostdownNative::STUB_ROOT
          else
            __rostdown_orig_initialize(source, *rest)
          end
        end
        klass.send(:alias_method, :__rostdown_orig_to_html, :to_html)
        klass.send(:define_method, :to_html) { @__rostdown_html || __rostdown_orig_to_html }
      end

      # The static tables embedded by `_rouge_native` were extracted
      # from THIS rouge release; using them with any other version
      # could silently diverge, so the fast path only engages when the
      # site's rouge/version.rb matches. (Bump together with
      # tools/dump_rouge_static_tables.rb regenerations.)
      STATIC_HL_ROUGE_VERSION = "4.7.0"

      # Static highlight fast path available? Requires the host fn
      # (a `_rouge_native` build) AND an on-disk rouge whose version
      # matches the embedded tables — checked by READING version.rb,
      # not by requiring the gem (avoiding the require is the point:
      # rouge eager-loads all 227 lexer files, ~200ms). The file check
      # also proves rouge exists, so highlighting is reproducing what
      # CRuby would do, not inventing it.
      def self.static_hl_ok?
        return @static_hl_ok unless @static_hl_ok.nil?
        @static_hl_ok =
          if defined?(__rubyrs_rouge_native_static_lex)
            begin
              path = nil
              $LOAD_PATH.each do |dir|
                candidate = File.join(dir, "rouge/version.rb")
                if File.exist?(candidate)
                  path = candidate
                  break
                end
              end
              !!(path && File.read(path).include?('"' + STATIC_HL_ROUGE_VERSION + '"'))
            rescue StandardError
              false
            end
          else
            false
          end
      end

      # kramdown's rouge plugin requires rouge lazily at first
      # highlight; the native path bypasses the plugin, so mirror that
      # here (this also fires rubyrs' _rouge_native hook, chaining the
      # carmine accelerator). Unavailable rouge → wholesale decline,
      # the pure-Ruby path then reproduces kramdown's own behavior.
      def self.rouge_available?
        return @rouge_available unless @rouge_available.nil?
        @rouge_available = begin
          unless defined?(::Rouge::Lexer)
            # With version-matched static tables we KNOW the on-disk
            # rouge layout: raise the lexer gate so `require "rouge"`
            # skips the eager 227-file lexer walk (~200ms); the rouge
            # shim then installs demand loading keyed off the same
            # flag. Without the version match the gate stays down and
            # rouge loads exactly as upstream.
            if static_hl_ok? && defined?(__rubyrs_rouge_native_lexer_gate)
              $__rubyrs_rouge_lexer_gate = true
              __rubyrs_rouge_native_lexer_gate(true)
            end
            require "rouge"
          end
          true
        rescue LoadError, SyntaxError
          false
        end
      end

      # Render source through rostdown. nil = declined (caller falls
      # back to the pure-Ruby parse).
      #
      # `static_hl_ok?` proves rouge EXISTS on disk (version.rb found)
      # without loading it, so the wholesale rouge_available? require
      # only happens when the static gate is closed — otherwise rouge
      # loads lazily on the first block the static path can't serve.
      def self.render(source, flavor = "jekyll")
        unless static_hl_ok? || rouge_available?
          $stderr.puts "[kd-hl-unavailable]" if ENV["RUBYRS_NATIVE_STATS"] == "2"
          return nil
        end
        sid = __rubyrs_kd_scan(source, flavor)
        if sid.nil?
          $stderr.puts "[kd-scan-decline] #{source[0, 60].inspect}" if ENV["RUBYRS_NATIVE_STATS"] == "2"
          return nil
        end
        begin
          i = 0
          n = __rubyrs_kd_count(sid)
          while i < n
            lang = __rubyrs_kd_lang(sid, i)
            code = __rubyrs_kd_code(sid, i)
            __rubyrs_kd_supply(sid, i, highlight_block(lang, code))
            i += 1
          end
          html = __rubyrs_kd_render(sid)
          if html.nil? && ENV["RUBYRS_NATIVE_STATS"] == "2"
            $stderr.puts "[kd-render-decline] engine declined post-supply"
          end
          html
        rescue StandardError => e
          if ENV["RUBYRS_NATIVE_STATS"] == "2"
            $stderr.puts "[kd-raise] #{e.class}: #{e.message.to_s[0, 80]}"
          end
          # Highlighting raised (or protocol misuse): free the session
          # and decline. kd_render frees on success, so a late abort on
          # an already-freed sid is a no-op.
          __rubyrs_kd_abort(sid)
          nil
        end
      end

      # Mirrors Kramdown::Converter::SyntaxHighlighter::Rouge.call for
      # :block with Jekyll's verified opts ({default_lang: "plaintext",
      # guess_lang: true} + the plugin's css_class fallback). nil means
      # "no highlighting" and the host renders kramdown's plain
      # <pre><code> branch, exactly like the plugin returning nil.
      def self.highlight_block(lang, code)
        # Static fast path: pre-extracted table + one-shot native lex,
        # rouge never loaded. nil (no table / callback rule / version
        # gate closed) escalates to the rouge-backed dynamic path.
        if static_hl_ok?
          html = __rubyrs_rouge_native_static_lex(lang, code)
          return html if html
        end
        return nil unless rouge_available?
        lexer = ::Rouge::Lexer.find_fancy(lang, code)
        return nil unless lexer
        formatter = ::Rouge::Formatters::HTMLLegacy.new(
          default_lang: "plaintext", guess_lang: true, css_class: "highlight"
        )
        formatter.format(lexer.lex(code))
      end
    end
    end # unless defined?(RostdownNative)

    # Patch whichever framework document class is defined now. Jekyll's
    # JekyllDocument is defined by the time `require "kramdown-parser-gfm"`
    # completes; Bridgetown's BridgetownDocument is defined when its
    # KramdownParser first triggers the (re-injecting) gfm require. install
    # is idempotent, so running both on every injection is safe.
    RostdownNative.install(::Kramdown::JekyllDocument, "jekyll") if defined?(::Kramdown::JekyllDocument)
    RostdownNative.install(::Kramdown::BridgetownDocument, "bridgetown") if defined?(::Kramdown::BridgetownDocument)
  end
end
