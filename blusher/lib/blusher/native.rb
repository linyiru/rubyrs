# frozen_string_literal: true

require "rbconfig"

module Blusher
  # The native boundary to the carmine engine. Coarse lex-or-decline contract:
  # `Native.lex(table_json, input)` returns either
  #   - an Array of [token_qualname, value] String pairs (carmine lexed it), or
  #   - nil (a callback rule blocks native lexing → caller falls back to rouge).
  #
  # Two backends, in preference order:
  #   1. the rb-sys/magnus native extension (`blusher.{bundle,so}`), which
  #      builds the token Array DIRECTLY as Ruby objects — no JSON round-trip.
  #      This is the release path and the only one that beats rouge.
  #   2. the `carmine-ffi` cdylib via Fiddle, marshaling tokens through JSON.
  #      A dependency-light bootstrap kept for environments without the
  #      precompiled ext; the JSON serialize/parse makes it a net loss vs rouge,
  #      so it exists for correctness/coverage, not speed.
  module Native
    DLEXT = (RbConfig::CONFIG["host_os"] =~ /darwin/ ? "dylib" : "so")

    # --- backend 1: magnus native extension --------------------------------
    # The loadable object's name must match the `Init_blusher` symbol and use
    # the platform's Ruby ext suffix (.bundle/.so) — `require` won't load a raw
    # cargo `.dylib`. `rake compile` stages it here from the cargo target dir.
    EXT_CANDIDATES = [
      # gem-installed by rake-compiler: lib/blusher/blusher.<dlext>
      File.expand_path("blusher.#{RbConfig::CONFIG["DLEXT"]}", __dir__),
      # dev: `rake compile` stages the cargo build at lib/blusher.<dlext>
      File.expand_path("../blusher.#{RbConfig::CONFIG["DLEXT"]}", __dir__),
      File.expand_path("../blusher.bundle", __dir__),
      File.expand_path("../blusher.so", __dir__),
    ].freeze

    @backend = nil

    ext = EXT_CANDIDATES.find { |p| File.exist?(p) }
    if ext
      require ext
      @backend = :ext
    end

    # --- backend 2: carmine-ffi via Fiddle ---------------------------------
    unless @backend
      require "fiddle"
      require "json"

      FFI_CANDIDATES = [
        File.expand_path("../../ext/libcarmine_ffi.#{DLEXT}", __dir__),
        File.expand_path("../../../target/release/libcarmine_ffi.#{DLEXT}", __dir__),
        File.expand_path("../../../target/debug/libcarmine_ffi.#{DLEXT}", __dir__),
      ].freeze

      path = FFI_CANDIDATES.find { |p| File.exist?(p) }
      unless path
        raise LoadError,
          "blusher: no native backend found (run `rake compile`). " \
          "Looked for the magnus ext in #{EXT_CANDIDATES} and carmine-ffi in #{FFI_CANDIDATES}"
      end

      LIB = Fiddle.dlopen(path)
      FFI_LEX = Fiddle::Function.new(
        LIB["carmine_lex"],
        [Fiddle::TYPE_VOIDP, Fiddle::TYPE_VOIDP, Fiddle::TYPE_SIZE_T],
        Fiddle::TYPE_VOIDP
      )
      FFI_FREE = Fiddle::Function.new(LIB["carmine_free"], [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOID)
      @backend = :ffi
    end

    class << self
      attr_reader :backend
    end

    # Both backends return [Token, value] pairs (or nil to decline). `qualname`
    # is rouge's qualname→Token Hash; the ext resolves Tokens in Rust (one pass,
    # no intermediate name-string array), the FFI path maps in Ruby.
    # `tag` keys the ext's parsed-table cache (parse+regex-compile is ~0.5ms and
    # input-independent, so it must not repeat per call).
    if @backend == :ext
      def self.lex(tag, table_json, input, qualname)
        Blusher::Engine.lex(tag, table_json, input, qualname)
      end

      # Fused lex + HTML formatting in Rust — returns one HTML String (or nil to
      # decline). Only the magnus ext supports this; it is the path that makes
      # blusher faster than rouge (no per-token Ruby object crosses the boundary).
      def self.format_html(tag, table_json, input, shortname)
        Blusher::Engine.format_html(tag, table_json, input, shortname)
      end
    else
      def self.lex(_tag, table_json, input, qualname)
        ptr = FFI_LEX.call(table_json, input, input.bytesize)
        begin
          result = JSON.parse(ptr.to_s)
        ensure
          FFI_FREE.call(ptr)
        end
        return nil unless result["status"] == "ok"
        error = qualname["Error"]
        result["tokens"].map { |name, val| [qualname[name] || error, val] }
      end

      # The FFI/JSON backend can't fuse profitably (it would re-cross the JSON
      # boundary); decline so the shim uses rouge's formatter on FFI tokens.
      def self.format_html(_tag, _table_json, _input, _shortname)
        nil
      end
    end

    def self.fused_html?
      @backend == :ext
    end
  end
end
