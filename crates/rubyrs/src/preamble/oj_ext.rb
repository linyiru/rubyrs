# `_oj` battery — stands in for the oj gem's `oj/oj` C extension.
# `Oj.dump` / `Oj.load` route to rubyrs's JSON, which is oj's `:compat`
# mode (standard JSON) — correct for plain-data dump/load (Hash / Array /
# String / Integer / Float / true / false / nil), the common "fast JSON
# drop-in" use of oj.
#
# DOCUMENTED DIVERGENCE: oj's DEFAULT `:object` mode encodes Symbols as
# `":sym"`, marshals arbitrary objects, and tracks circular refs — none
# of that is modelled here (Symbols dump as standard JSON strings, an
# unsupported object raises). `mode:` and most options are accepted and
# ignored. Apps using oj purely as a fast JSON parser/generator work;
# apps relying on `:object`-mode round-tripping of Ruby objects do not.
require "json"

module Oj
  class << self
    # `Oj.dump(obj, options = nil)` → JSON string. Options are accepted
    # for source compatibility but only standard-JSON output is produced.
    def dump(obj, options = nil)
      _ = options
      JSON.generate(obj)
    end
    alias_method :to_json, :dump
    alias_method :generate, :dump

    # `Oj.load(json, options = nil)` → Ruby object. `symbol_keys: true`
    # symbolizes Hash keys (oj's option); other options are ignored.
    def load(json, options = nil)
      sym = options.is_a?(Hash) && (options[:symbol_keys] || options[:symbolize_names])
      JSON.parse(json, symbolize_names: !!sym)
    end
    alias_method :safe_load, :load
    alias_method :strict_load, :load
    alias_method :compat_load, :load

    def load_file(path, options = nil)
      load(File.read(path), options)
    end

    def dump_file(path, obj, options = nil)
      File.write(path, dump(obj, options))
    end

    # `Oj.default_options` / `Oj.default_options=` — oj's global option
    # hash. Stored but inert (output is always standard JSON).
    def default_options
      @default_options ||= { mode: :compat }
    end

    def default_options=(opts)
      @default_options = opts
    end
  end
end
