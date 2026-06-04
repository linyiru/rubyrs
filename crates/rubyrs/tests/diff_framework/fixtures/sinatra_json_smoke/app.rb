# sinatra_json_smoke app — exercises real
# sinatra-contrib-4.2.1/lib/sinatra/json.rb vendored 1:1 under
# vendor/sinatra/json.rb. The gem's last line is `Base.helpers
# JSON`, but modular form needs the explicit `helpers Sinatra::JSON`
# line (real Sinatra installs onto Application classic class, not
# every Sinatra::Base subclass).
#
# JSON serialisation order matters for byte-diff parity. CRuby's
# `JSON.generate` preserves Hash insertion order; rubyrs's
# stdlib_vendor/json.rb does the same. Scenarios use ordered
# literal Hashes / Arrays so both sides emit identical bytes.

require_relative "sinatra_compat"

class SinatraJsonSmokeApp < Sinatra::Base
  helpers Sinatra::JSON

  # Default encoder + content_type. The gem's
  # `Base.set :json_encoder do; ::MultiJson; end` installs the
  # encoder on Sinatra::Base; the `Base.set :json_content_type,
  # :json` installs the content type. Both reach this subclass
  # via inheritance.
  get "/flat" do
    json a: 1, b: "two", c: true
  end

  # Array as top-level.
  get "/array" do
    json [1, 2, 3, "four"]
  end

  # Nested Hash + Array.
  get "/nested" do
    json users: [
      { name: "Ada", age: 36 },
      { name: "Bob", age: 28 },
    ]
  end

  # Per-call content_type override via the `:content_type` option
  # — exercises the `resolve_content_type(options)` branch.
  get "/custom_type" do
    json({ ok: true }, content_type: "application/vnd.api+json")
  end

  # Empty Hash / Array — degenerate shapes that JSON encoders
  # still handle deterministically.
  get "/empty_hash" do
    json({})
  end

  get "/empty_array" do
    json []
  end

  # Primitives — JSON allows them as top-level since RFC 7159.
  get "/primitive_string" do
    json "hello"
  end

  get "/primitive_int" do
    json 42
  end

  get "/primitive_null" do
    json nil
  end

  get "/" do
    "backend: #{SERVER_BACKEND}"
  end
end

HARNESS_RUN_APP.call(SinatraJsonSmokeApp)
