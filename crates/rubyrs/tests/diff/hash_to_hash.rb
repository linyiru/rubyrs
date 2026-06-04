# `Hash#to_hash` — the explicit-conversion alias gems use as the
# duck-type probe for "I really am a Hash". Surfaced by
# sinatra-contrib/LinkHeader's `urls.last.respond_to?(:to_hash) ?
# urls.pop : {}` pattern — without `to_hash` on real Hashes, the
# pop didn't happen and the opts Hash leaked into the URL list.

h = { a: 1, b: 2 }
p h.respond_to?(:to_hash)
p h.to_hash
p h.to_hash == h
p h.to_hash.equal?(h)

# Empty Hash.
e = {}
p e.respond_to?(:to_hash)
p e.to_hash

# Non-Hash values DON'T respond — that's the whole point.
p "foo".respond_to?(:to_hash)
p [].respond_to?(:to_hash)
p 42.respond_to?(:to_hash)
p :sym.respond_to?(:to_hash)
p nil.respond_to?(:to_hash)
