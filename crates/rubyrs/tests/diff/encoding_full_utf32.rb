def kls(&b); b.call; "NO-RAISE"; rescue => ex; ex.class.name; end
s = "hé"
le = s.encode("UTF-32LE")
p le.encoding.name
p le.bytes
p le.encode("UTF-8") == s
be = s.encode("UTF-32BE")
p be.bytes
p be.encode("UTF-8") == s
# astral (single 4-byte unit, no surrogate)
e = "😀"
p e.encode("UTF-32LE").bytes
p e.encode("UTF-32BE").bytes
p e.encode("UTF-32LE").encode("UTF-8") == e
# length / bytesize / valid_encoding?
p "ab".encode("UTF-32LE").length
p "ab".encode("UTF-32LE").bytesize
p "ab".encode("UTF-32LE").valid_encoding?
# Encoding.find + constants
p Encoding.find("UTF-32LE").name
p Encoding.find("utf-32be").name
p Encoding::UTF_32LE.name
# BOM form
u = s.encode("UTF-32")
p u.encoding.name
p u.bytes
p u.encode("UTF-8") == s
p "".encode("UTF-32").bytes
le_bom = [0xFF,0xFE,0x00,0x00, 0x68,0x00,0x00,0x00].pack("C*").force_encoding("UTF-32")
p le_bom.encode("UTF-8")
# invalid: length not mult of 4 / codepoint out of range / no BOM
p kls { [0x00,0x00].pack("C*").force_encoding("UTF-32LE").encode("UTF-8") }
p kls { [0x00,0x00,0x11,0x00].pack("C*").force_encoding("UTF-32LE").encode("UTF-8") }
p kls { [0x68,0x00,0x00,0x00].pack("C*").force_encoding("UTF-32").encode("UTF-8") }
