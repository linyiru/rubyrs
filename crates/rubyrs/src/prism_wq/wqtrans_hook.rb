# rubyrs native whitequark-translation hook (prism_wq).
#
# Injected by the require handler right after the prism gem's
# "prism/translation/parser" loads. Routes `Prism::Translation::Parser#tokenize`
# — the seam RuboCop's ProcessedSource drives — through the native
# `__rubyrs_wqtrans_tokenize` host fn (compiler + builder + lexer ports in
# Rust), falling back to the gem's interpreted translation whenever the native
# port declines a file. RUBYRS_WQTRANS_NO_NATIVE=1 is the kill switch.

module Prism
  module Translation
    class Parser
      WQTRANS_NATIVE = !ENV["RUBYRS_WQTRANS_NO_NATIVE"] && defined?(__rubyrs_wqtrans_tokenize) ? true : false

      alias_method :__rubyrs_interp_tokenize, :tokenize

      def tokenize(source_buffer, recover = false)
        if WQTRANS_NATIVE && !recover
          native = __rubyrs_wq_native_tokenize(source_buffer)
          return native if native
        end
        __rubyrs_interp_tokenize(source_buffer, recover)
      end

      # The native attempt; nil when the port declines (caller falls back).
      # Public-ish so the deep-equal corpus probe can exercise both paths
      # explicitly.
      def __rubyrs_wq_native_tokenize(source_buffer)
        return nil unless __rubyrs_wq_native_applicable?
        @source_buffer = source_buffer
        options_blob = Prism.send(:dump_options, send(:prism_options))
        result = __rubyrs_wqtrans_tokenize(
          source_buffer,
          source_buffer.source,
          options_blob,
          source_buffer.name.to_s,
          source_buffer.first_line,
          version
        )
        return nil unless result

        ast, comments, tokens, diags = result
        # Replay diagnostics through the engine in the interpreted order
        # (prism errors, prism warnings, builder diagnostics). On rubyrs the
        # engine's all_errors_are_fatal raises Parser::SyntaxError at the
        # first error — exactly where the interpreted build would have.
        diags.each { |row| diagnostics.process(__rubyrs_wq_diagnostic(row)) }
        [ast, comments, tokens]
      ensure
        @source_buffer = nil
      end

      # The native port models exactly the RuboCop shape: a stock
      # Parser33/34/40/41 over the real Prism, building for
      # RuboCop::AST::BuilderPrism with its documented emit-flag
      # configuration. Anything else falls back.
      def __rubyrs_wq_native_applicable?
        return false unless instance_of?(Parser33) || instance_of?(Parser34) ||
                            instance_of?(Parser40) || instance_of?(Parser41)
        return false unless @parser.equal?(Prism)
        b = builder
        return false unless defined?(::RuboCop::AST::BuilderPrism) &&
                            b.instance_of?(::RuboCop::AST::BuilderPrism)
        klass = b.class
        return false unless klass.emit_forward_arg && klass.emit_match_pattern
        return false if klass.emit_lambda || klass.emit_procarg0 || klass.emit_encoding ||
                        klass.emit_index || klass.emit_arg_inside_procarg0 || klass.emit_kwargs
        return false unless b.emit_file_line_as_literals
        true
      end

      # Row: [prism?, level, reason, message, args_flat, begin, end, hl_flat].
      def __rubyrs_wq_diagnostic(row)
        prism_row, level, reason, message, args_flat, bpos, epos, hl_flat = row
        range = ::Parser::Source::Range.new(@source_buffer, bpos, epos)
        return PrismDiagnostic.new(message, level, reason, range) if prism_row

        args = {}
        i = 0
        while i < args_flat.length
          args[args_flat[i]] = args_flat[i + 1]
          i += 2
        end
        highlights = []
        i = 0
        while i < hl_flat.length
          highlights << ::Parser::Source::Range.new(@source_buffer, hl_flat[i], hl_flat[i + 1])
          i += 2
        end
        Diagnostic.new(level, reason, args, range, highlights)
      end
    end
  end
end
