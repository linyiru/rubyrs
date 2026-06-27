# The full Hash API (`[]=`, `[]`, `key?`, `delete`, `fetch`) honors keys that
# override `hash`/`eql?` — equal keys collapse, lookups find them. A
# `compare_by_identity` Hash keys strictly on identity and never calls hash/eql
# (zeitwerk's Cref::Map stores non-hashable module objects this way).
class K
  def hash; 7; end
  def eql?(other); other.is_a?(K); end
end
h = {}
h[K.new] = 1
h[K.new] = 2
p h.size                 # 1
p h[K.new]               # 2
p h.key?(K.new)          # true
p h.fetch(K.new)         # 2
h.delete(K.new)
p h.size                 # 0

# compare_by_identity: a module whose hash is overridden (wrong arity) is keyed
# by identity, so no hash() call happens.
m = Module.new do
  def self.hash(_) = nil
end
idh = {}
idh.compare_by_identity
idh[m] = :ok
p idh[m]                 # :ok
p idh.size               # 1
p idh.compare_by_identity?  # true
