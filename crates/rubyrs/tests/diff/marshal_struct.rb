# Struct (`S`-tag) binary marshalling: CRuby-byte-compatible dump,
# deep copy, nested structs, shared-object links, anonymous fallback.
S = Struct.new(:a, :b)
s = S.new(1, "x")
p Marshal.dump(s).bytes
r = Marshal.load(Marshal.dump(s))
p [r.a, r.b, r.class.name]
# deep copy independence
orig = S.new([1, 2], "y")
copy = Marshal.load(Marshal.dump(orig))
copy.a << 99
p orig.a
p orig.equal?(copy)
# nil member round-trips
p Marshal.load(Marshal.dump(S.new(1, nil))).to_a
# nested struct
T = Struct.new(:inner)
p Marshal.load(Marshal.dump(T.new(S.new(10, 20)))).inner.to_a
# shared struct reconstructs shared identity
sh = S.new(1, 2)
rr = Marshal.load(Marshal.dump([sh, sh]))
p rr[0].equal?(rr[1])
# struct nested in a hash
p Marshal.load(Marshal.dump({s: S.new(9, 8)})).fetch(:s).a
# anonymous struct (not const-assigned) → TypeError on dump
anon = Struct.new(:x).new(5)
begin
  Marshal.dump(anon)
  puts "NO-RAISE"
rescue TypeError
  puts "anon: TypeError"
end
