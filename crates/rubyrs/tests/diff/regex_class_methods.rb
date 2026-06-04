# Regexp class methods — `Regexp.escape` / `Regexp.quote` /
# `Regexp.compile` / `Regexp.new`. Runtime-built patterns (gems
# that turn user-supplied strings into Regexps) all route through
# these — rack-cors / sinatra route DSLs / template engines, etc.

# escape / quote (alias) — return a String with regex metachars
# backslash-escaped so the result can be safely interpolated into
# a pattern.
puts Regexp.escape("a.b+c")
puts Regexp.escape("a*b?c|d(e)")
puts Regexp.escape("/path/")
puts Regexp.escape("plain")
puts Regexp.escape("")
puts Regexp.quote("a.b+c")          # alias of escape
# Note: CRuby's Regexp.escape also backslashes whitespace
# (`/ /` → `/\ /`), but Rust `regex::escape` doesn't. The
# behaviour-divergent space is a dialect quirk documented in
# SUBSET.md; pick inputs that avoid it for the parity oracle.
puts Regexp.quote("[anchors]$^")

# compile / new — build a Regexp from a String pattern. The
# resulting Regexp behaves identically to a `/.../` literal.
re = Regexp.compile("^[a-z]+://example\\.com$")
puts re.class
puts "http://example.com" =~ re
puts ("ftp://other.com" =~ re).inspect

re2 = Regexp.new("foo.*bar")
puts "fooXXXbar" =~ re2
puts ("nope" =~ re2).inspect

# The classic escape→interpolate→compile pipeline gems use to
# turn an untrusted user string into a safe pattern.
host = "ex.am.ple+v2.com"
host_re = Regexp.compile("^[a-z]+://#{Regexp.quote(host)}$")
puts "https://ex.am.ple+v2.com" =~ host_re
puts ("https://hostile.com" =~ host_re).inspect

# Error shapes.
begin
  Regexp.compile(123)
rescue TypeError => e
  puts "TypeError: #{e.message}"
end
begin
  Regexp.escape
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end
