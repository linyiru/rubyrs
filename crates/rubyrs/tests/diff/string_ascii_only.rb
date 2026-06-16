# String#ascii_only? — native, cached, byte-level. Covers the cache
# invalidation on mutation (the cache must reset when content changes)
# and the empty / non-ASCII cases.
p "abc".ascii_only?                 # true
p "hello world 123 !@#$%^&*()".ascii_only?  # true
p "café".ascii_only?                # false (é is multibyte)
p "".ascii_only?                    # true
p "\t\n\r".ascii_only?              # true (control chars are ASCII)

s = +"ascii"
p s.ascii_only?                     # true (computes + caches)
p s.ascii_only?                     # true (hits cache)
s << [0xe9].pack("C").force_encoding("UTF-8")
p s.ascii_only?                     # false — cache invalidated by <<

t = +"x"
t.replace("overé")
p t.ascii_only?                     # false after replace

u = +"abcabc"
u.gsub!("b", "X")
p u.ascii_only?                     # true (still ASCII after gsub!)
