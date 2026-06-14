# Regexp#match?(nil) returns false (CRuby treats nil as "no match"
# rather than raising). rack's request IP filter calls
# `trusted_proxies.match?(ip)` where ip can be nil for some forwarded
# entries (spec_request "deals with proxies").
p(/x/.match?(nil))                       # false
p(/\d+/.match?(nil))                     # false
p(Regexp.union(/a/, /b/).match?(nil))    # false

# a real ip-filter style use
trusted = Regexp.union(/\A127\.0\.0\.1\z/, /\A::1\z/)
filter = ->(ip) { trusted.match?(ip) }
p filter.call("127.0.0.1")               # true
p filter.call(nil)                       # false
p filter.call("8.8.8.8")                 # false
