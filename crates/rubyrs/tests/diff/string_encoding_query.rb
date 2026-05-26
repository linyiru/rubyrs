# String#valid_encoding? and #encoding — query-side completion of
# the existing encode/force_encoding no-op stubs. rubyrs stores
# raw bytes viewed via String::from_utf8_lossy, so the observable
# character stream is always well-formed UTF-8. Both queries
# return that.
#
# DIVERGENCE from CRuby: real CRuby tracks per-string encoding
# tags and can return false from `valid_encoding?` for malformed
# UTF-8 byte sequences. We can't model that without dragging in
# the encoding system. Real codebases (tilt at template.rb:120,
# many others) use these as guards before deciding whether to
# raise; the no-op return matches the always-valid case which
# is the only one our representation can ever be in.

# Always-true valid_encoding? — the load-bearing case from tilt.
puts "hello".valid_encoding?
puts "".valid_encoding?
puts "中文".valid_encoding?
puts "with #{:interp}".valid_encoding?

# encoding returns a String name. CRuby returns an Encoding
# object; we don't model Encoding. `.to_s` on both yields the
# same string in CRuby, so we mirror that surface.
puts "hello".encoding.to_s
puts "中文".encoding.to_s

# Round-trip with the existing encode / force_encoding no-ops.
s = "data".force_encoding("UTF-8")
puts s.valid_encoding?
puts s.encoding.to_s
t = "more".encode("UTF-8")
puts t.valid_encoding?
puts t.encoding.to_s

# Direct `==` between `encoding` and a String diverges (CRuby's
# Encoding object isn't equal to its String name; we return a
# String so the comparison is true). Real codebases typically
# write `.encoding.to_s == "UTF-8"`, which works in both:
puts("hello".encoding.to_s == "UTF-8")
