# rubyrs Prism backend (ADR 0036 Slice 1 / productionized).
#
# Replaces the prism C extension (`prism/prism`), which rubyrs cannot dlopen — it is a
# CRuby-ABI `.bundle`. The prism C library itself IS linked into rubyrs (via the `ruby-prism`
# crate, used for rubyrs's own frontend); the `__rubyrs_prism_serialize_parse*` host fns call
# `pm_serialize_parse(_lex)` and return its serialized wire blob. This file builds
# `Prism.parse` / `Prism.parse_lex` on those host fns + the prism gem's pure-Ruby
# `Prism::Serialize` deserializer (loaded here), so `require "prism"` works and RuboCop's
# `parser_prism` engine runs without the C extension.
#
# Injected by rubyrs's `require` handler when a script (e.g. rubocop-ast) requires
# "prism/prism" with the prism gem on the load path (which supplies node/parse_result/serialize).
require "prism/serialize"

module Prism
  # prism.rb sets BACKEND = :CEXT before requiring "prism/prism"; don't clobber it.
  BACKEND = :RUBYRS unless const_defined?(:BACKEND)

  class << self
    def dump(source, **options)
      __rubyrs_prism_serialize_parse(source)
    end

    def parse(source, **options)
      Prism::Serialize.load_parse(source, __rubyrs_prism_serialize_parse(source), options.fetch(:freeze, false))
    end

    def parse_lex(source, **options)
      Prism::Serialize.load_parse_lex(source, __rubyrs_prism_serialize_parse_lex(source), options.fetch(:freeze, false))
    end

    def lex(source, **options)
      Prism::Serialize.load_lex(source, __rubyrs_prism_serialize_parse_lex(source), options.fetch(:freeze, false))
    end

    def parse_file(filepath, **options)
      parse(File.read(filepath), **options)
    end

    def parse_lex_file(filepath, **options)
      parse_lex(File.read(filepath), **options)
    end
  end
end
