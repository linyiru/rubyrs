# String#sub / #gsub over a regex must preserve the receiver's raw bytes
# when it's ASCII-8BIT (BINARY). A lossy UTF-8 round-trip turns every
# invalid byte into a 3-byte U+FFFD, which corrupts AND grows the result
# (rack's multipart parser strips a trailing boundary from a binary file
# body with `body.sub(@body_regex_at_end, '')`).

# BINARY (ASCII-8BIT) subject
s = ("\xC3\xC3hello\xFF\xFF" * 3).b + "END"
r = s.sub(/END\z/, '')
puts "sub binary: #{r.bytesize} (expected #{s.bytesize - 3}) enc=#{r.encoding}"

g = ("\xFFx\xFFy" * 5).b
r2 = g.gsub(/x/, 'Z')
puts "gsub binary: #{r2.bytesize} (expected #{g.bytesize}) enc=#{r2.encoding}"

# valid UTF-8 (incl. multibyte) still works via the normal path
v = "héllo wörld"
puts "sub valid: #{v.sub(/wörld/, 'WORLD')}"
puts "gsub valid: #{v.gsub(/l/, 'L')}"

# backreferences over a binary subject
b = "\xFF[keep]\xFF".b
puts "backref: #{b.sub(/\[(\w+)\]/, '<\1>').bytesize} (expected #{b.bytesize - 2 + 2})"
