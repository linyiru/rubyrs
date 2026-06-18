# String#slice! with char-index ranges on BINARY / non-UTF-8 bytes must
# index by byte (rack-session decrypt: data.slice!(-32..-1) on a
# Base64-decoded cookie). Build bytes that are invalid UTF-8.
data = [0x80, 0xff, 0xfe, 0x01, 0x02, 0x90, 0xab, 0xcd].pack("C*")
p data.bytesize
tail = data.slice!(-3..-1)
p tail.bytes
p data.bytes
head = data.slice!(0, 2)
p head.bytes
p data.bytes
# single negative index
b = [0xc3, 0x28, 0xa0].pack("C*")
p b.slice!(-1).bytes
p b.bytes
