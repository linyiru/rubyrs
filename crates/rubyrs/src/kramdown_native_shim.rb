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
if defined?(__rubyrs_kd_scan) && defined?(::Kramdown::JekyllDocument) &&
   !::Kramdown::JekyllDocument.method_defined?(:__rostdown_orig_initialize)
  module Kramdown
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
      IGNORED = ["toc_levels", "footnote_nr", "show_warnings", "coderay"].freeze

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
            return false unless v == HL_OPTS || v == HL_OPTS_SYM
            next
          end
          return false unless REQUIRED.key?(k) && REQUIRED[k] == v
        end
        REQUIRED.each_key do |k|
          return false unless options.key?(k)
        end
        true
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
      def self.render(source)
        return nil unless static_hl_ok? || rouge_available?
        sid = __rubyrs_kd_scan(source)
        return nil if sid.nil?
        begin
          i = 0
          n = __rubyrs_kd_count(sid)
          while i < n
            lang = __rubyrs_kd_lang(sid, i)
            code = __rubyrs_kd_code(sid, i)
            __rubyrs_kd_supply(sid, i, highlight_block(lang, code))
            i += 1
          end
          __rubyrs_kd_render(sid)
        rescue StandardError
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
  end

  class ::Kramdown::JekyllDocument
    alias_method :__rostdown_orig_initialize, :initialize
    def initialize(source, options = {})
      @__rostdown_html = nil
      if ::Kramdown::RostdownNative.eligible?(options)
        html = ::Kramdown::RostdownNative.render(source)
        if html
          # Skip the Ruby parse entirely; Jekyll only touches #to_html
          # and #warnings afterwards.
          @__rostdown_html = html
          @options = options
          @warnings = []
          @root = nil
          return
        end
      end
      __rostdown_orig_initialize(source, options)
    end

    alias_method :__rostdown_orig_to_html, :to_html
    def to_html
      @__rostdown_html || __rostdown_orig_to_html
    end
  end
end
