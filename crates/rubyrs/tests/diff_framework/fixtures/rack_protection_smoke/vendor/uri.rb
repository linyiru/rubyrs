# Minimal `require 'uri'` shim. base.rb's `referrer` helper
# parses HTTP_REFERER via `URI.parse(ref).host`; JsonCsrf
# triggers that path on every JSON response. CRuby's real URI
# loads from stdlib; rubyrs resolves to this shim.
#
# Only `.host` is reached on the parsed URI in this fixture.
# Parser handles `scheme://[user:pass@]host[:port][/path]`
# forms — the common case for an HTTP Referer header. Returns
# a Generic with nil host for empty / unparseable input (the
# wrapped helper already guards with `rescue
# URI::InvalidURIError` so a nil-host return matches the
# catch-fallthrough path).
module URI
  class InvalidURIError < StandardError; end

  class Generic
    attr_reader :host
    def initialize(host)
      @host = host
    end
  end

  def self.parse(raw)
    raw = raw.to_s
    return Generic.new(nil) if raw.empty?
    idx = raw.index("//")
    rest = idx ? raw[(idx + 2)..] : raw
    %w[/ ? #].each do |sep|
      cut = rest.index(sep)
      rest = rest[0...cut] if cut
    end
    if (at = rest.index("@"))
      rest = rest[(at + 1)..]
    end
    if (colon = rest.index(":"))
      rest = rest[0...colon]
    end
    Generic.new(rest.empty? ? nil : rest)
  end
end
