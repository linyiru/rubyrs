# String#[] / #slice on an ASCII-8BIT (BINARY) receiver must index by
# BYTES and keep the BINARY tag. The char path routes through a
# UTF-8-lossy view that U+FFFD-mangles non-UTF-8 bytes — that
# corrupted StringIO#read and Zlib over gzip bodies (vm/string.rs
# binary fast-path). Zero require so the default-features Coverage
# build exercises it. Binary built via pack (literals would UTF-8-
# substitute high bytes).
def t(l); r = begin; yield.inspect; rescue => e; "#{e.class}: #{e.message[0, 60]}"; end; puts "#{l}: #{r}"; end

bin = (0..20).to_a.pack("C*")           # 21 bytes, values 0..20
t("enc")        { bin.encoding.to_s }
t("idx0")       { bin[0].bytes }        # [0]
t("idx5")       { bin[5].bytes }        # [5]
t("idx neg")    { bin[-1].bytes }       # [20]
t("idx oob")    { bin[100] }            # nil
t("int,len")    { bin[3, 4].bytes }     # [3,4,5,6]
t("int,len neg"){ bin[-3, 2].bytes }    # [18,19]
t("int,0")      { bin[5, 0].bytes }     # []
t("range from") { bin[10..].bytes }     # [10..20]
t("range incl") { bin[2..5].bytes }     # [2,3,4,5]
t("range excl") { bin[2...5].bytes }    # [2,3,4]
t("range to")   { bin[..3].bytes }      # [0,1,2,3]
t("range neg")  { bin[-3..].bytes }     # [18,19,20]
t("slice tag")  { bin[2..5].encoding.to_s }   # BINARY preserved
t("slice m tag"){ bin[3, 4].encoding.to_s }

# The mangle case: high bytes (invalid UTF-8) must survive byte-sliced.
hi = [0xff, 0x00, 0xfe, 0x80, 0x41].pack("C*")
t("hi all")     { hi[0..].bytes }       # [255,0,254,128,65]
t("hi mid")     { hi[1, 2].bytes }      # [0, 254]
t("hi bytesize"){ hi[0..].bytesize }    # 5 (NOT expanded by U+FFFD)
t("hi enc")     { hi[1..3].encoding.to_s }
