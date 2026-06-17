# MatchData#begin / #end / #offset (char offsets) + #bytebegin /
# #byteend / #byteoffset (byte offsets), for integer and named group
# indices, including multibyte subjects and non-participating groups.

# Multibyte, all-positional groups: begin/end report CHARACTER offsets.
m = "héllo wörld".match(/(l+)o (w)/)
p m.begin(0)
p m.end(0)
p m.offset(0)
p m.offset(1)
p m.offset(2)
# Byte variants on the same multibyte match.
p m.bytebegin(0)
p m.byteend(0)
p m.byteoffset(0)
p m.byteoffset(1)

# All-named groups: begin/offset accept Symbol or String.
mn = "héllo wörld".match(/(?<a>l+)o (?<b>w)/)
p mn.begin(:a)
p mn.offset(:a)
p mn.byteoffset("b")

# Non-participating optional group → nil / [nil, nil].
m2 = "ac".match(/(a)(x)?(c)/)
p m2.begin(2)
p m2.end(2)
p m2.offset(2)
p m2.byteoffset(2)
p m2.offset(3)

# Out-of-range and negative indices raise IndexError; unknown name too.
def rescued
  yield
rescue => e
  e.class
end
p(rescued { m2.begin(9) })
p(rescued { m2.begin(-1) })
p(rescued { mn.begin(:zzz) })

# Offsets resolve through $~ set by =~, String#[], and the match block.
"hello world" =~ /(o) (w)/
p $~.begin(0)
p $~.offset(1)
"xxabc"[/a(b)c/]
p $~.begin(1)
p $~.byteoffset(1)
"hello".match(/l(l)/) { |md| p md.offset(1); p md.begin(0) }
