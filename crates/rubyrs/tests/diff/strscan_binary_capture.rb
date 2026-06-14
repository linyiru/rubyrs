# StringScanner over an ASCII-8BIT subject: a capture group read via
# `ss[n]` must preserve raw bytes (ASCII-8BIT), not lossy U+FFFD. This is
# exactly how rack's multipart parser extracts the content-disposition
# head, which can contain an invalid filename byte (spec_multipart
# "filename containing invalid characters").
require 'strscan'

s = "Content-Disposition: form-data; name=\"f\"; filename=\"inv\xC3.txt\"\r\n".b
ss = StringScanner.new(s)
ss.scan_until(/(.*?\r\n)/m)
head = ss[1]
p head.bytes.last(12)        # ...includes 195 (\xC3), not 239,191,189
p head.encoding.to_s         # "ASCII-8BIT"

# named-ish: positional group with the invalid byte preserved
ss2 = StringScanner.new("a\xFFb".b)
ss2.scan_until(/(a.b)/n)
p ss2[1].bytes               # [97, 255, 98]
