# URI.parse rejects characters outside the RFC 2396 grammar — raw
# non-ASCII (must be %-encoded), spaces, controls, `<>` — anywhere in
# the reference, with URI::InvalidURIError. rack's Lint uses this to
# flag a non-ASCII SERVER_NAME: `URI.parse("http://#{name}/") rescue
# false`.
require 'uri'

def probe(u)
  URI.parse(u)
  "OK"
rescue URI::InvalidURIError
  "RAISE"
end

# accepted (valid ASCII URIs)
p probe("http://example.com/path")
p probe("http://ok.com/?q=1&r=2")
p probe("/just/path")
p probe("mailto:a@b.com")
p probe("http://user:pw@host:80/p?q#f")
p probe("http://h/%20already%20escaped")
p probe("https://[::1]:8080/v6")        # IPv6 host brackets

# rejected (non-ASCII / disallowed chars)
p probe("http://exámple.com/")          # non-ASCII host
p probe("http://例/")                    # CJK host
p probe("http://ok.com/á")              # non-ASCII path
p probe("http://ex ample/")             # space
p probe("http://ex<>ample/")            # angle brackets

# the rescue-false idiom rack's Lint uses
p (URI.parse("http://#{"ሴ"}/") rescue false)
p !!(URI.parse("http://example.com/") rescue false)   # true (valid parses)

# Kernel#URI raises too
p (URI("http://badሴ/") rescue $!.class)
