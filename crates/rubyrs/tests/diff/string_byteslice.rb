# String#byteslice — slice by BYTE offsets, preserving the receiver's
# encoding (CRuby keeps the original encoding even when the cut lands
# inside a multibyte character). The (index) and (index, length) forms
# already worked; this pins the Range form (inclusive / exclusive /
# endless / beginless / negative endpoints), which had been missing.
s = "héllo"   # h é(2 bytes) l l o = 6 bytes

# index / (index, length) forms
p s.byteslice(0, 3)            # "hé"
p s.byteslice(1)               # "\xC3" (single byte)
p s.byteslice(-2, 2)           # "lo"
p s.byteslice(100)             # nil
p s.byteslice(0, 0)            # ""

# Range forms
p s.byteslice(2..4)            # inclusive
p s.byteslice(2...4)           # exclusive
p s.byteslice(2..)             # endless
p s.byteslice(..3)             # beginless
p s.byteslice(3..2)            # "" (empty span, begin <= len)
p s.byteslice(10..)            # nil (begin > len)
p s.byteslice(0..-1)           # whole (negative end)
p s.byteslice(-3..-1)          # last 3 bytes

# encoding preserved across a multibyte-splitting cut
p s.byteslice(2..4).encoding.name   # "UTF-8"

# binary receiver keeps ASCII-8BIT
b = "\xff\x00\x41".b
p b.byteslice(0..1)
p b.byteslice(0..1).encoding.name   # "ASCII-8BIT"
