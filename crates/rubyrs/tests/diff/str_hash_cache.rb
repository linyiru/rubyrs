# StrCell cached ruby_hash — invalidation contract: every content
# mutation goes through borrow_mut (the only mutation door), which
# clears the cache. Pins hash/eql consistency across mutation,
# duplication, freezing and encoding-flavoured content.

# 1. Same-content strings hash equal (cached vs fresh probe).
a = "jekyll-data-key"
b = "jekyll-data-" + "key"
h = { a => 1 }
puts h[b]
puts a.hash == b.hash

# 2. Hot probe loop (cache warms), then mutate the PROBE string —
#    its hash must change to the new content's.
probe = "color"
table = { "color" => "red", "colour" => "blue" }
100.times { table[probe] }
puts table[probe]
probe << "x"   # "colorx" — in-place mutation through borrow_mut
puts table[probe].inspect
probe.sub!(/x\z/, "")
probe.sub!(/^col/, "col")  # no-op rewrite still routes through mutation
puts table[probe]
puts table["colour"]

# 3. dup/clone get fresh cells; freezing doesn't change the hash.
base = "frozen-key"
copy = base.dup
puts base.hash == copy.hash
frozen = base.dup.freeze
puts base.hash == frozen.hash
hh = { frozen => :ok }
puts hh["frozen-key"]

# 4. A mutated string's hash matches a fresh string of the new
#    content (NOT pinned here: whether Hash dups string keys at
#    insert — CRuby dup+freezes, a separate pre-existing divergence
#    unrelated to this cache).
k = "mutant"
k << "-grew"
puts k.hash == "mutant-grew".hash

# 5. Repeated .hash calls are stable.
s = "stability"
h1 = s.hash
h2 = s.hash
puts h1 == h2
s << "!"
puts s.hash == h1
