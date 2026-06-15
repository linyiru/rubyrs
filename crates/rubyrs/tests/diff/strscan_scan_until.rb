# StringScanner's non-slicing native search (binary/ASCII buffers) must
# stay byte-identical to the slice path: scan_until/check_until/
# skip_until, scan-position \A anchoring, captures, pre/post_match, and
# repeated scans over the same buffer (the multipart driver's shape).
require "strscan"

def run(str)
  ss = StringScanner.new(str)
  body = /(?:\r\n|\A)--bnd(?:\r\n|--)/m
  out = []
  out << ss.scan_until(body)            # opening boundary (\A branch)
  out << ss.pos
  out << ss.scan_until(/(.*?\r\n)\r\n/m) # a head, capture group 1
  out << ss[1]
  out << ss.check_until(body)           # check (no advance)
  out << ss.pos
  out << ss.scan_until(body)            # consume boundary at \A (scan-pos)
  out << ss.pre_match
  out << ss.matched
  out << ss.eos?
  out
end

# binary buffer
p run("--bnd\r\nA: 1\r\n\r\nv\r\n--bnd--".b)
# ASCII UTF-8 buffer (the all-ASCII multipart case)
p run("--bnd\r\nA: 1\r\n\r\nv\r\n--bnd--")
# scan-position \A: boundary exactly at pos, no preceding EOL
s2 = StringScanner.new("xx--bnd\r\ny".b); s2.pos = 2
p s2.scan_until(/(?:\r\n|\A)--bnd(?:\r\n|--)/m)
p s2.pre_match
# skip_until + rest
s3 = StringScanner.new("aaa--bnd\r\nzzz".b)
p s3.skip_until(/--bnd\r\n/)
p s3.rest
# no-match
p StringScanner.new("nothing here".b).scan_until(/--bnd/)
