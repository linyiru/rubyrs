# Real binary Marshal.dump for the common-tag subset: load(dump(x)) is
# now a genuine DEEP COPY (mutating the copy leaves the original
# untouched) and the dump bytes are CRuby-4.8 byte-compatible.
orig = [1, [2, 3], {a: 4}]
copy = Marshal.load(Marshal.dump(orig))
copy[1] << 99
p orig
p copy
p orig.equal?(copy)
# round-trips across the subset
p Marshal.load(Marshal.dump(nil)).nil?
p Marshal.load(Marshal.dump([true, false, 42, -1000000, 1.5, :s, "x"]))
p Marshal.load(Marshal.dump({"a" => [1, 2], :b => "x"})) == {"a" => [1, 2], :b => "x"}
# shared substructure reconstructs shared identity (object link)
s = "shared"
r = Marshal.load(Marshal.dump([s, s]))
p r[0].equal?(r[1])
# self-referential cycle survives (no infinite recursion)
a = [1]
a << a
rc = Marshal.load(Marshal.dump(a))
p rc[0]
p rc[1].equal?(rc)
# byte-compatible dumps for the non-float subset
p Marshal.dump([1, 2]).bytes
p Marshal.dump("hi").bytes
p Marshal.dump(:ab).bytes
p Marshal.dump({a: 1}).bytes
p Marshal.dump("").bytes
p Marshal.dump("\xff\x00".b).bytes
p Marshal.dump("a".force_encoding("US-ASCII")).bytes
# encoding round-trips
p Marshal.load(Marshal.dump("héllo")).encoding.name
p Marshal.load(Marshal.dump("a".force_encoding("US-ASCII"))).encoding.name
p Marshal.load(Marshal.dump("\xff\x00".b)).encoding.name
# bignum falls back to the token but still round-trips by value
big = 10**30
p Marshal.load(Marshal.dump(big)) == big
# proc still raises TypeError (fallback path's dumpability probe)
begin; Marshal.dump(proc {}); rescue TypeError; puts "proc: TypeError"; end
