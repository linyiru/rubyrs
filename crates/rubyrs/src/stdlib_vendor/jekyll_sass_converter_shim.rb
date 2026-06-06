# jekyll-sass-converter shim (ADR 0026 blessed reimpl).
#
# The real gem requires `sass-embedded`, which drives a native
# dart-sass binary over a google-protobuf stdio protocol (Open3
# subprocess) — none of which rubyrs can run. Instead we define the
# converter classes jekyll registers, and `convert` delegates SCSS→CSS
# to the Rust `RubyrsSass.compile` host primitive (the `sass` battery,
# grass-backed). With the battery built in, real `.scss`/`.sass`
# sources compile; without it, `RubyrsSass.compile` raises a clear
# "feature absent" error (and plain, non-SCSS sites build regardless).
#
# Discovery: P3 Jekyll spike — jekyll.rb:195 `require
# "jekyll-sass-converter"`.

# `convert` below delegates to `RubyrsSass.compile(scss) -> css`,
# defined in the preamble and wired in vm/dispatch.rs to
# crate::sass::compile (the SassBackend seam).

module Jekyll
  module Converters
    class Scss < Converter
      EXTENSION_PATTERN = %r!^\.scss$!i
      SyntaxError = Class.new(ArgumentError) unless defined?(SyntaxError)

      safe true
      priority :low

      def matches?(ext)
        ext =~ self.class::EXTENSION_PATTERN
      end

      def output_ext(_ext)
        ".css"
      end

      def convert(content)
        RubyrsSass.compile(content)
      rescue StandardError => e
        raise self.class::SyntaxError, e.message
      end

      # Hooks the real converter registers wire up source-map page
      # objects; no-ops here since convert never runs for plain sites.
      def associate_page(_page); end
      def dissociate_page(_page); end
    end

    class Sass < Scss
      EXTENSION_PATTERN = %r!^\.sass$!i
    end
  end
end

module JekyllSassConverter
end
