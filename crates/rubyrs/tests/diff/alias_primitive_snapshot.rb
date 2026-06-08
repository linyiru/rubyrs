# `alias`/`alias_method` snapshot the CURRENT method, so redefining the
# original afterward does NOT change what the alias calls. For an alias
# of a PRIMITIVE method on a subclass, rubyrs synthesises a forwarder;
# that forwarder must run the PRIMITIVE (not late-bind to a later
# redefinition), else `alias own_keys keys; def keys; own_keys ...; end`
# recurses forever. This is rouge's InheritableHash pattern
# (rouge/util.rb:33).

# (1) alias of a USER method, then redefine the original.
class C
  def greet; "original"; end
  alias old_greet greet
  def greet; "new+" + old_greet; end
end
p C.new.greet                      # "new+original"

# (2) alias of a PRIMITIVE (Hash#keys) on a Hash subclass, then override.
class MyHash < Hash
  alias own_keys keys
  def keys; own_keys + [:extra]; end
end
h = MyHash.new; h[:a] = 1; h[:b] = 2
p h.keys.sort_by(&:to_s)           # [:a, :b, :extra]
p h.own_keys.sort_by(&:to_s)       # [:a, :b]  (primitive snapshot, no :extra)

# (3) alias_method form, same shape.
class MyHash2 < Hash
  alias_method :orig_size, :size
  def size; orig_size + 100; end
end
h2 = MyHash2.new; h2[:x] = 1
p h2.size                          # 101
p h2.orig_size                     # 1

# (4) the forwarder reflects later mutations of the receiver (it's not a
#     value snapshot — it re-runs the primitive each call).
class MyHash3 < Hash
  alias snapshot_keys keys
end
h3 = MyHash3.new; h3[:a] = 1
first = h3.snapshot_keys
h3[:b] = 2
p first.sort_by(&:to_s)            # [:a]   (Array captured earlier)
p h3.snapshot_keys.sort_by(&:to_s) # [:a, :b]  (re-runs primitive)
