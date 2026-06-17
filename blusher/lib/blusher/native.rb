# frozen_string_literal: true

require "fiddle"
require "json"
require "rbconfig"

module Blusher
  # The native boundary to the carmine engine (the `carmine-ffi` cdylib).
  # Coarse lex-or-decline contract: `lex` returns a parsed JSON Hash —
  # {"status"=>"ok","tokens"=>[[name,val],…]} / {"status"=>"decline"} /
  # {"status"=>"error",…}. Input is passed length-delimited so embedded NUL
  # bytes survive.
  #
  # (FFI/Fiddle is the bootstrap; the release path is an rb-sys/magnus native
  # extension with precompiled cross-platform binaries — see README.)
  module Native
    DLEXT = RbConfig::CONFIG["host_os"] =~ /darwin/ ? "dylib" : "so"

    CANDIDATES = [
      # packaged inside the gem (release: precompiled per platform)
      File.expand_path("../../ext/libcarmine_ffi.#{DLEXT}", __dir__),
      # dev: the cargo workspace target dir (rubyrs monorepo)
      File.expand_path("../../../target/release/libcarmine_ffi.#{DLEXT}", __dir__),
      File.expand_path("../../../target/debug/libcarmine_ffi.#{DLEXT}", __dir__),
    ].freeze

    path = CANDIDATES.find { |p| File.exist?(p) }
    raise LoadError, "blusher: carmine-ffi native lib not found (run `rake compile`); looked in #{CANDIDATES}" unless path

    LIB = Fiddle.dlopen(path)
    LEX = Fiddle::Function.new(
      LIB["carmine_lex"],
      [Fiddle::TYPE_VOIDP, Fiddle::TYPE_VOIDP, Fiddle::TYPE_SIZE_T],
      Fiddle::TYPE_VOIDP
    )
    FREE = Fiddle::Function.new(LIB["carmine_free"], [Fiddle::TYPE_VOIDP], Fiddle::TYPE_VOID)

    def self.lex(table_json, input)
      ptr = LEX.call(table_json, input, input.bytesize)
      begin
        JSON.parse(ptr.to_s)
      ensure
        FREE.call(ptr)
      end
    end
  end
end
