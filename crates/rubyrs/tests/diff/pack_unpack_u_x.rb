# pack/unpack 'U' (UTF-8 codepoints) and 'x' (skip/pad bytes). Surfaced by
# builder (`[cp].pack('U')`, `seq.pack('U*')`) and tzinfo
# (`data.unpack('a4 a x15 NNNNNN')`).

# pack 'U' — codepoints -> UTF-8 bytes
p [0x41, 0x42].pack("U*").bytes              # [65, 66]
p [0x2764].pack("U").bytes                   # [226, 157, 164] (❤)
p [0x4e2d, 0x6587].pack("UU").force_encoding("UTF-8")  # "中文"
p [104, 105].pack("U*").force_encoding("UTF-8")        # "hi"

# unpack 'U' — UTF-8 bytes -> codepoints
p "AB".unpack("U*")                          # [65, 66]
p "中文".unpack("U*")                         # [20013, 25991]
p "héllo".unpack("U3")                       # [104, 233, 108]

# unpack 'x' — skip bytes
p "abcdef".unpack("x2 a*")                    # ["cdef"]
p "ABCDxxxxEFGH".unpack("a4 x4 a4")           # ["ABCD", "EFGH"]

# pack 'x' — null padding
p "ab".bytes.pack("C*x3").bytes               # [97, 98, 0, 0, 0]
