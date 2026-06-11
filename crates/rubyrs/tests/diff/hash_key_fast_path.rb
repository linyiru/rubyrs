# Hash key-probe fast path (`try_fast_index`, vm/dispatch.rs) —
# `key?` / `has_key?` / `include?` / `member?` on a plain Hash
# short-circuit dispatch. Every gate must agree with CRuby:
# hit/miss/nil-value, mixed key types, defaulted hashes (probes
# never consult defaults), subclass tag fall-through, late reopen
# (lumped flag — ANY of the four names overridden turns all off),
# and Array#include?/member? keeping VALUE-search semantics.

# All four spellings, hit and miss, nil-valued key counts as present.
h = { "a" => 1, "b" => nil, 3 => :three }
p h.key?("a")
p h.has_key?("a")
p h.include?("a")
p h.member?("a")
p h.key?("b")
p h.key?("zzz")
p h.include?("zzz")

# Mixed key types through the same probe.
m = { 1 => :i, 1.5 => :f, true => :t, nil => :n, :s => :sym, "k" => :str }
p m.key?(1)
p m.key?(1.5)
p m.key?(true)
p m.key?(nil)
p m.key?(:s)
p m.key?("k")
p m.key?(2)
p m.include?(:s)
p m.member?(nil)

# Defaulted hashes: probes must NOT consult the default value or
# run the default block.
dv = Hash.new(42)
p dv.key?("x")
p dv.include?("x")
db = Hash.new { |hh, k| hh[k] = "blk-#{k}" }
p db.key?("x")
p db.size          # the probe didn't insert
p db["x"]          # but [] does
p db.key?("x")

# Subclass instances fall through to the subclass-override gate.
class MyHash < Hash
  def key?(k)
    "sub-key-#{super}"
  end
end
mh = MyHash.new
mh["w"] = 9
p mh.key?("w")
p mh.include?("w") # not overridden -> canonical, via slow path

# Hot use FIRST, then reopen ONE of the four names — the lumped
# flag must turn the whole probe arm off, and the override wins.
1000.times { h.key?("a"); h.include?("a") }
class Hash
  def include?(k)
    "hash-include-reopen"
  end
end
p h.include?("a")
p h.key?("a")      # not overridden -> still canonical via slow path
p h.member?("a")

# Array include?/member? are VALUE searches — never the key probe.
a = [10, "s", nil, false]
p a.include?(10)
p a.include?("s")
p a.include?(nil)
p a.include?(false)
p a.include?(99)
p a.member?(10)
p a.member?(99)
