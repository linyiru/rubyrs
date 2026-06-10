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
      IGNORED = ["toc_levels", "footnote_nr", "show_warnings", "coderay"].freeze

      def self.eligible?(options)
        return false unless options.is_a?(Hash)
        options.each do |k, v|
          next if IGNORED.include?(k)
          if k == "syntax_highlighter_opts"
            return false unless v == HL_OPTS
            next
          end
          return false unless REQUIRED.key?(k) && REQUIRED[k] == v
        end
        REQUIRED.each_key do |k|
          return false unless options.key?(k)
        end
        true
      end

      # kramdown's rouge plugin requires rouge lazily at first
      # highlight; the native path bypasses the plugin, so mirror that
      # here (this also fires rubyrs' _rouge_native hook, chaining the
      # carmine accelerator). Unavailable rouge → wholesale decline,
      # the pure-Ruby path then reproduces kramdown's own behavior.
      def self.rouge_available?
        return @rouge_available unless @rouge_available.nil?
        @rouge_available = begin
          require "rouge" unless defined?(::Rouge::Lexer)
          true
        rescue LoadError, SyntaxError
          false
        end
      end

      # Render source through rostdown. nil = declined (caller falls
      # back to the pure-Ruby parse).
      def self.render(source)
        return nil unless rouge_available?
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
