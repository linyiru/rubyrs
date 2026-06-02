# Minimal `multi_json` shim — rubyrs side only. The real
# multi_json gem is an adapter layer over `oj` / `yajl` /
# stdlib JSON / etc.; we only need the `dump` entry the
# vendored sinatra-jsonp file calls. Routes through
# rubyrs's built-in JSON canon (src/stdlib_vendor/json.rb,
# auto-detects the _json_native accelerator). On the CRuby
# side this shim is NOT loaded — the real multi_json gem
# resolves via rubygems and produces byte-identical
# JSON.generate output for the deterministic subset the
# fixture exercises (primitives + flat Hash).
require "json"

module MultiJson
  # Canonical Ruby module-function shape now that rubyrs's
  # bare `module_function` auto-mirrors subsequent defs onto
  # the module's singleton class. Matches what most real Ruby
  # libraries (multi_json gem itself included) write.
  module_function

  def dump(obj, opts = {})
    if opts && opts[:pretty]
      JSON.pretty_generate(obj)
    else
      JSON.generate(obj)
    end
  end
end
