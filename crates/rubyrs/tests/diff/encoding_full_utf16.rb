# --- UTF-16LE / UTF-16BE: encode, decode, round-trip, bytes ---
s = "héllo"
le = s.encode("UTF-16LE")
p le.encoding.name
p le.bytes
p le.encode("UTF-8") == s
be = s.encode("UTF-16BE")
p be.bytes
p be.encode("UTF-8") == s
# astral plane (surrogate pair)
e = "😀"
p e.encode("UTF-16LE").bytes
p e.encode("UTF-16BE").bytes
p e.encode("UTF-16LE").encode("UTF-8") == e
# ascii-only
p "abc".encode("UTF-16LE").bytes
# length / bytesize / valid_encoding? on a UTF-16 string
p "ab".encode("UTF-16LE").length
p "ab".encode("UTF-16LE").bytesize
p "ab".encode("UTF-16LE").valid_encoding?
# Encoding.find + constants
p Encoding.find("UTF-16LE").name
p Encoding.find("utf-16be").name
p Encoding::UTF_16LE.name
# --- BOM-form "UTF-16" ---
u = s.encode("UTF-16")
p u.encoding.name
p u.bytes
p u.encode("UTF-8") == s
p "".encode("UTF-16").bytes
# decode with explicit LE BOM
le_bom = [0xFF,0xFE, 0x68,0x00].pack("C*").force_encoding("UTF-16")
p le_bom.encode("UTF-8")
# --- invalid byte sequences raise InvalidByteSequenceError (class only) ---
def kls(&b); b.call; "NO-RAISE"; rescue => ex; ex.class.name; end
p kls { [0xD8,0x3D].pack("C*").force_encoding("UTF-16BE").encode("UTF-8") }
p kls { [0x00].pack("C*").force_encoding("UTF-16LE").encode("UTF-8") }
p kls { [0x00,0x68].pack("C*").force_encoding("UTF-16").encode("UTF-8") }
