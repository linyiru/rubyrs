# Collection-index fast path (`try_fast_index`, vm/dispatch.rs) —
# every gate the fast path can take must agree with CRuby:
# hit/miss/nil-value, negative & out-of-range Array indices,
# default-value/default-block fall-through, subclass override,
# late reopen (method_gen invalidation AFTER the path ran hot),
# and non-Int Array args (slices) falling through.

# Plain hits and misses, nil-value vs missing key.
h = { "a" => 1, "b" => nil }
p h["a"]
p h["b"]
p h["zzz"]
p h.key?("b")

# Mixed key types through the same fast path.
m = { 1 => :i, 1.5 => :f, true => :t, nil => :n, :s => :sym, "k" => :str }
p m[1]
p m[1.5]
p m[true]
p m[nil]
p m[:s]
p m["k"]
p m[2]

# Array: positive, negative wrap, out of range both directions.
a = [10, 20, 30]
p a[0]
p a[2]
p a[-1]
p a[-3]
p a[3]
p a[-4]

# Non-Int args fall through to the canonical arms.
p a[0, 2]
p a[1..2]
p a[0...2]

# Default-block hash: miss must run the block (fall-through), and
# the mutating idiom must actually insert.
d = Hash.new { |hh, k| hh[k] = "blk-#{k}" }
p d["x"]
p d.key?("x")
p d["x"]

# Hot use FIRST, then a subclass overrides `[]` — the tag gate.
class MyHash < Hash
  def [](k)
    "sub-#{super}"
  end
end
mh = MyHash.new
mh["w"] = 9
1000.times { h["a"] }
p mh["w"]

# Hot use FIRST, then reopen Hash#[] / Array#[] — the
# method_gen-revalidated override flags must turn the path off.
1000.times { h["a"]; a[0] }
class Hash
  def [](k)
    "hash-reopen"
  end
end
p h["a"]
class Array
  def [](i)
    "array-reopen"
  end
end
p a[0]

# ---- []= write twins ----

# Insert + overwrite + expression value.
w = {}
p (w["a"] = 1)
w["a"] = 2
w[:sym] = 3
w[5] = 4
p w

# Array in-bounds (positive + negative), growth padding, append.
g = [10, 20, 30]
g[1] = 99
g[-1] = 88
p g
g[5] = 77          # growth: pads with nils (slow-path fall-through)
p g
g[g.length] = 66   # append at exact end
p g

# Defaulted hash still writes normally.
dd = Hash.new(0)
dd["x"] = 5
p dd["x"]
p dd["missing"]

# Hot use FIRST, then reopen []= only — the per-name flag must turn
# the WRITE path off while reads keep their canonical semantics.
hw = { "a" => 1 }
1000.times { hw["a"] = 2; hw["a"] }
class Hash
  def []=(k, v)
    "hash-set-reopen"
  end
end
# NOTE: deliberately NOT pinning the assignment-expression value
# here — rubyrs has a pre-existing divergence where a user
# `[]=` override's return value leaks as the expression value
# (CRuby always evaluates `x[k] = v` to the RHS).
hw["zzz"] = 9
p hw.key?("zzz")   # the override didn't insert -> false
p hw["a"]          # reads still canonical
class Array
  def []=(i, v)
    "array-set-reopen"
  end
end
aw = [1, 2]
aw[0] = 5
p aw               # override didn't write -> [1, 2]
