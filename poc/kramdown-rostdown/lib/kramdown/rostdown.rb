# frozen_string_literal: true

require "ffi"
require "rbconfig"
require "kramdown"
require_relative "rostdown/version"

module Kramdown
  # Drop-in accelerator for the kramdown gem, backed by the Rust
  # `rostdown` renderer (FFI). Require this file and existing
  # `Kramdown::Document.new(src, opts).to_html` calls render through
  # rostdown when the options + source fall inside its byte-identical
  # subset, and fall back to pure-Ruby kramdown otherwise — no code
  # change at the call site.
  #
  # This mirrors rubyrs' in-VM `_kramdown_native` accelerator (the shim
  # in `crates/rubyrs/src/kramdown_native_shim.rb`), ported to stock
  # CRuby over the C ABI in `ext/`.
  module Rostdown
    # ---- FFI binding to the rostdown cdylib --------------------------
    module Native
      extend FFI::Library

      def self.locate_lib
        return ENV["KRAMDOWN_ROSTDOWN_LIB"] if ENV["KRAMDOWN_ROSTDOWN_LIB"]

        ext = RbConfig::CONFIG["host_os"] =~ /darwin/ ? "dylib" : "so"
        base = File.expand_path("../../ext/target/release", __dir__)
        File.join(base, "libkramdown_rostdown_cabi.#{ext}")
      end

      ffi_lib locate_lib

      # scan(src_ptr, src_len, gfm, auto_ids, codespan_hl, default_plaintext)
      attach_function :rd_scan, %i[pointer size_t bool bool bool bool], :int64
      attach_function :rd_block_count, [:int64], :int64
      attach_function :rd_block_lang, %i[int64 int64], :pointer
      attach_function :rd_block_code, %i[int64 int64], :pointer
      attach_function :rd_supply, %i[int64 int64 pointer size_t], :void
      attach_function :rd_render, [:int64], :pointer
      attach_function :rd_abort, [:int64], :void
      attach_function :rd_string_free, [:pointer], :void
    end

    # The four rostdown knobs derived from a kramdown options hash.
    Profile = Struct.new(:gfm, :auto_ids, :codespan_hl, :default_plaintext)

    SMART_QUOTES = %w[lsquo rsquo ldquo rdquo].freeze

    # Two highlighter shapes we can reproduce byte-identically:
    #   {} (community / kramdown-core / bare GFM) → bare <code>, no-lang
    #      fences stay plain.
    #   {default_lang: plaintext, guess_lang: true} (Jekyll's setup) →
    #      code spans + no-lang fences get the language-plaintext class.
    # Both key spellings, since kramdown normalises string keys to
    # symbols once any document takes the pure-Ruby path.
    JEKYLL_SHO = [
      { default_lang: "plaintext", guess_lang: true },
      { "default_lang" => "plaintext", "guess_lang" => true },
    ].freeze

    class << self
      # Map a *raw* kramdown options hash to a Profile, or nil when the
      # options sit outside rostdown's byte-identical subset (caller then
      # runs pure-Ruby kramdown). Conservative by construction: anything
      # we are not certain reproduces exactly returns nil.
      def profile_for(raw_options)
        opts = ::Kramdown::Options.merge(raw_options || {})

        return nil unless opts[:entity_output] == :as_char
        return nil unless opts[:smart_quotes] == SMART_QUOTES
        return nil unless (opts[:typographic_symbols] || {}).empty?
        return nil unless (opts[:header_offset] || 0).zero?
        return nil unless opts[:template].to_s.empty?
        return nil if opts[:auto_id_stripping]
        return nil if opts[:transliterated_header_ids]
        return nil if opts[:header_links]
        return nil if opts[:remove_line_breaks_for_cjk]
        return nil unless [nil, ""].include?(opts[:auto_id_prefix])
        return nil unless (opts[:link_defs] || {}).empty?
        return nil unless [nil, :rouge].include?(opts[:syntax_highlighter])

        input = (opts[:input] || "kramdown").to_s
        gfm =
          case input
          when "GFM"      then true
          when "kramdown" then false
          else return nil
          end
        # The kramdown-core parser ignores hard_wrap; the GFM parser
        # turns it into <br>, which rostdown never emits.
        return nil if gfm && opts[:hard_wrap] != false
        # rostdown bakes the default GFM quirks (paragraph_end on, smart
        # typography on). Any other gfm_quirks set (e.g. paragraph_end
        # disabled, or :no_auto_typographic which kills smart quotes)
        # changes rendering — decline.
        return nil if gfm && Array(opts[:gfm_quirks]).sort != [:paragraph_end]

        sho = opts[:syntax_highlighter_opts] || {}
        if sho.empty?
          codespan_hl = false
          default_plaintext = false
        elsif JEKYLL_SHO.include?(sho)
          codespan_hl = true
          default_plaintext = true
        else
          return nil
        end

        Profile.new(gfm, !!opts[:auto_ids], codespan_hl, default_plaintext)
      end

      # Render `source` through rostdown for an eligible Profile. Returns
      # the HTML, or nil if rostdown declined the document or any code
      # block could not be highlighted (caller falls back to kramdown).
      def render(source, profile)
        return nil unless source.is_a?(String)

        bytes = source.b
        buf = FFI::MemoryPointer.new(:char, bytes.bytesize)
        buf.put_bytes(0, bytes) unless bytes.empty?
        sid = Native.rd_scan(buf, bytes.bytesize, profile.gfm, profile.auto_ids,
                             profile.codespan_hl, profile.default_plaintext)
        return nil if sid.negative?

        begin
          n = Native.rd_block_count(sid)
          return abort_render(sid) if n.negative?

          i = 0
          while i < n
            lang = take_string(Native.rd_block_lang(sid, i))
            code = take_string(Native.rd_block_code(sid, i))
            return abort_render(sid) if lang.nil? || code.nil?

            html = highlight_block(lang, code, profile)
            supply(sid, i, html)
            i += 1
          end
          take_string(Native.rd_render(sid))
        rescue StandardError
          abort_render(sid)
        end
      end

      # ---- internals ------------------------------------------------
      def stats
        @stats ||= Hash.new(0)
      end

      def stat(key)
        stats[key] += 1
      end

      private

      def supply(sid, i, html)
        if html.nil?
          Native.rd_supply(sid, i, FFI::Pointer::NULL, 0)
        else
          b = html.b
          hbuf = FFI::MemoryPointer.new(:char, b.bytesize)
          hbuf.put_bytes(0, b) unless b.empty?
          Native.rd_supply(sid, i, hbuf, b.bytesize)
        end
      end

      def abort_render(sid)
        Native.rd_abort(sid)
        nil
      end

      def take_string(ptr)
        return nil if ptr.null?

        s = ptr.read_string
        Native.rd_string_free(ptr)
        s.force_encoding("UTF-8")
      end

      # Mirrors Kramdown::Converter::SyntaxHighlighter::Rouge.call for a
      # :block with the profile's effective opts. nil = "no highlighting"
      # (host renders the plain <pre><code> branch, exactly as the plugin
      # returning nil does).
      def highlight_block(lang, code, profile)
        default_lang = profile.default_plaintext ? "plaintext" : nil
        guess_lang = profile.default_plaintext
        lang = nil if lang && lang.empty?
        return nil unless lang || default_lang || guess_lang
        return nil unless rouge_available?

        lexer = ::Rouge::Lexer.find_fancy(lang || default_lang, code)
        return nil if lexer.nil? || (lexer.tag == "plaintext" && !guess_lang)

        ::Rouge::Formatters::HTMLLegacy.new(
          default_lang: default_lang, guess_lang: guess_lang, css_class: "highlight"
        ).format(lexer.lex(code))
      end

      def rouge_available?
        return @rouge_available unless @rouge_available.nil?

        @rouge_available =
          begin
            require "rouge"
            true
          rescue LoadError, SyntaxError
            false
          end
      end
    end

    # ---- the zero-code-change hook ----------------------------------
    # Capture the genuine parser BEFORE prepending, so the lazy fallback
    # can build the AST without re-entering our patched initialize.
    ORIG_INITIALIZE = ::Kramdown::Document.instance_method(:initialize)

    module DocumentPatch
      def initialize(source, options = {})
        profile = ::Kramdown::Rostdown.profile_for(options)
        html = profile && ::Kramdown::Rostdown.render(source, profile)
        if html
          @__rd_html = html
          @__rd_source = source
          @__rd_options_arg = options
          @options = ::Kramdown::Options.merge(options).freeze
          @warnings = []
          @root = nil
          ::Kramdown::Rostdown.stat(:native)
        else
          ::Kramdown::Rostdown.stat(profile ? :decline : :ineligible)
          super
        end
      end

      # Intercept only `to_html` on the native path; every other to_X (and
      # any other dynamic dispatch) builds the real tree first so stock
      # kramdown behaviour is preserved unchanged.
      def method_missing(id, *args, &block)
        if defined?(@__rd_html) && @__rd_html
          return @__rd_html if id == :to_html

          __rostdown_ensure_parsed!
        end
        super
      end

      # Reading the AST forces a real parse.
      def root
        __rostdown_ensure_parsed! if defined?(@__rd_html) && @__rd_html
        super
      end

      private

      def __rostdown_ensure_parsed!
        return unless defined?(@__rd_html) && @__rd_html

        src = @__rd_source
        opts = @__rd_options_arg
        @__rd_html = nil
        ::Kramdown::Rostdown::ORIG_INITIALIZE.bind(self).call(src, opts)
      end
    end

    ::Kramdown::Document.prepend(DocumentPatch)

    if ENV["KRAMDOWN_ROSTDOWN_STATS"]
      at_exit do
        line = stats.sort_by { |k, _| k.to_s }.map { |k, v| "#{k}=#{v}" }.join(" ")
        warn "[kramdown-rostdown] #{line}"
      end
    end
  end
end
