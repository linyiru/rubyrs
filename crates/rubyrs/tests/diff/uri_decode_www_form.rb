# URI.decode_www_form_component — `%XX` → byte, `+` → space, else
# verbatim; ArgumentError when a `%` lacks two trailing hex digits.
# Backed by a native byte scan (the pure-Ruby per-byte fallback made
# rack's urlencoded POST parsing of large bodies pathologically slow).
require 'uri'

p URI.decode_www_form_component("hello+world")
p URI.decode_www_form_component("a%20b%2Fc")
p URI.decode_www_form_component("%E2%9C%93")        # UTF-8 ✓
p URI.decode_www_form_component("plain")
p URI.decode_www_form_component("")
p URI.decode_www_form_component("%41%42%43")         # ABC
p URI.decode_www_form_component("100%25+done")       # "100% done"
p URI.decode_www_form_component("a+b%2Bc")           # "a b+c" (literal +)
p URI.decode_www_form_component("key=val&x=y")       # decode leaves =,& as-is

# lowercase + uppercase hex both decode
p URI.decode_www_form_component("%ff%FF").bytes

# result is tagged UTF-8 by default
d = URI.decode_www_form_component("ok")
p d.encoding.name

# invalid %-encodings raise ArgumentError with the original string
["%2", "%zz", "abc%", "%g0", "%"].each do |bad|
  begin
    URI.decode_www_form_component(bad)
    puts "NO RAISE: #{bad.inspect}"
  rescue ArgumentError => e
    puts "ArgumentError: #{e.message}"
  end
end
