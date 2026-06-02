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
  # `def self.foo` rather than `module_function` — rubyrs's
  # Tier-1 module-visibility model doesn't generate the
  # singleton-class fallback `module_function` installs (the
  # call-site `MultiJson.dump(...)` would resolve to "undefined
  # method `dump' for Class"). `def self.X` produces a method
  # callable via `MultiJson.X(...)` on both runtimes — same
  # external contract as `module_function`'d code.
  def self.dump(obj, opts = {})
    if opts && opts[:pretty]
      JSON.pretty_generate(obj)
    else
      JSON.generate(obj)
    end
  end
end
