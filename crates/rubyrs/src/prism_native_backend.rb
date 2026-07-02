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

  # ADR 0036 Slice 2: prefer the NATIVE materializer — the host fn parses AND builds the
  # Prism::ParseResult object graph in Rust, skipping the interpreted Serialize
  # deserializer (the dominant per-file parse cost). Only when the loaded gem is the
  # exact version the native decode table was generated against (the gem's node.rb ivar
  # layout is baked into that table); the host fn independently pins the WIRE version.
  # The materializer returns nil to DECLINE (unknown encoding, missing class, freeze
  # requested, ...) — callers below then fall back to the gem's own Serialize path,
  # whose behavior is the spec. RUBYRS_PRISM_NO_NATIVE=1 is the kill switch (debugging /
  # A-B measurement against the interpreted deserializer).
  NATIVE_MATERIALIZE = !ENV["RUBYRS_PRISM_NO_NATIVE"] &&
                       defined?(__rubyrs_prism_materialize_parse) &&
                       Serialize::MAJOR_VERSION == 1 &&
                       Serialize::MINOR_VERSION == 9 &&
                       Serialize::PATCH_VERSION == 0

  class << self
    def dump(source, **options)
      __rubyrs_prism_serialize_parse(source, dump_options(options))
    end

    def parse(source, **options)
      if NATIVE_MATERIALIZE && !options.fetch(:freeze, false)
        result = __rubyrs_prism_materialize_parse(source, dump_options(options))
        return result if result
      end
      Prism::Serialize.load_parse(source, __rubyrs_prism_serialize_parse(source, dump_options(options)), options.fetch(:freeze, false))
    end

    def parse_lex(source, **options)
      if NATIVE_MATERIALIZE && !options.fetch(:freeze, false)
        result = __rubyrs_prism_materialize_parse_lex(source, dump_options(options))
        return result if result
      end
      Prism::Serialize.load_parse_lex(source, __rubyrs_prism_serialize_parse_lex(source, dump_options(options)), options.fetch(:freeze, false))
    end

    def lex(source, **options)
      # NB: load_lex expects pm_serialize_lex's wire layout (tokens first, NO header row)
      # — the parse_lex blob is a different format and misparses here.
      Prism::Serialize.load_lex(source, __rubyrs_prism_serialize_lex(source, dump_options(options)), options.fetch(:freeze, false))
    end

    def parse_file(filepath, **options)
      options[:filepath] = filepath
      parse(File.read(filepath), **options)
    end

    def parse_lex_file(filepath, **options)
      options[:filepath] = filepath
      parse_lex(File.read(filepath), **options)
    end

    private

    # The next three methods are ported VERBATIM from the prism gem's FFI backend
    # (prism/ffi.rb, v1.9.0 — matching the linked C). They serialize the keyword options
    # (filepath / version / partial_script / encoding, which RuboCop's
    # `Prism::Translation::Parser#prism_options` passes) into the `pm_options` wire format
    # that `pm_serialize_parse(_lex)` reads. They live in ffi.rb only because the FFI/C-ext
    # boundary lives there; the format itself is pure data, not parser logic — so honouring
    # the options the translation passes (instead of NULL/defaults) needs this here.

    def dump_options_command_line(options)
      command_line = options.fetch(:command_line, "")
      raise ArgumentError, "command_line must be a string" unless command_line.is_a?(String)

      command_line.each_char.inject(0) do |value, char|
        case char
        when "a" then value | 0b000001
        when "e" then value | 0b000010
        when "l" then value | 0b000100
        when "n" then value | 0b001000
        when "p" then value | 0b010000
        when "x" then value | 0b100000
        else raise ArgumentError, "invalid command_line option: #{char}"
        end
      end
    end

    def dump_options_version(version)
      current = version == "current"

      case current ? RUBY_VERSION : version
      when nil, "latest"
        0 # Handled in pm_parser_init
      when /\A3\.3(\.\d+)?\z/
        1
      when /\A3\.4(\.\d+)?\z/
        2
      when /\A3\.5(\.\d+)?\z/, /\A4\.0(\.\d+)?\z/
        3
      when /\A4\.1(\.\d+)?\z/
        4
      else
        if current
          raise CurrentVersionError, RUBY_VERSION
        else
          raise ArgumentError, "invalid version: #{version}"
        end
      end
    end

    def dump_options(options)
      template = +""
      values = []

      template << "L"
      if (filepath = options[:filepath])
        values.push(filepath.bytesize, filepath.b)
        template << "A*"
      else
        values << 0
      end

      template << "l"
      values << options.fetch(:line, 1)

      template << "L"
      if (encoding = options[:encoding])
        name = encoding.is_a?(Encoding) ? encoding.name : encoding
        values.push(name.bytesize, name.b)
        template << "A*"
      else
        values << 0
      end

      template << "C"
      values << (options.fetch(:frozen_string_literal, false) ? 1 : 0)

      template << "C"
      values << dump_options_command_line(options)

      template << "C"
      values << dump_options_version(options[:version])

      template << "C"
      values << (options[:encoding] == false ? 1 : 0)

      template << "C"
      values << (options.fetch(:main_script, false) ? 1 : 0)

      template << "C"
      values << (options.fetch(:partial_script, false) ? 1 : 0)

      template << "C"
      values << (options.fetch(:freeze, false) ? 1 : 0)

      template << "L"
      if (scopes = options[:scopes])
        values << scopes.length

        scopes.each do |scope|
          locals = nil
          forwarding = 0

          case scope
          when Array
            locals = scope
          when Scope
            locals = scope.locals

            scope.forwarding.each do |forward|
              case forward
              when :*     then forwarding |= 0x1
              when :**    then forwarding |= 0x2
              when :&     then forwarding |= 0x4
              when :"..." then forwarding |= 0x8
              else raise ArgumentError, "invalid forwarding value: #{forward}"
              end
            end
          else
            raise TypeError, "wrong argument type #{scope.class.inspect} (expected Array or Prism::Scope)"
          end

          template << "L"
          values << locals.length

          template << "C"
          values << forwarding

          locals.each do |local|
            name = local.name
            template << "L"
            values << name.bytesize

            template << "A*"
            values << name.b
          end
        end
      else
        values << 0
      end

      values.pack(template)
    end
  end
end
