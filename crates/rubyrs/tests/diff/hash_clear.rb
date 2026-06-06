# Hash#clear removes all pairs and returns self. Discovery: P3 Jekyll
# spike — Liquid's strainer.rb clears its filter cache.
h = {a: 1, b: 2, c: 3}
r = h.clear
p h
p r.equal?(h)      # returns the same Hash
p h.empty?
p h.size
h[:x] = 9          # usable after clear
p h
e = {}
p e.clear          # already-empty
