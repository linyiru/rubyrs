# `Kernel#Array(x)` coerces non-nil / non-Array values by
# wrapping them in a fresh one-element Array: `Array("hi")` →
# `["hi"]`. Under STRESS_GC=1 this site is delicate: the wrapped
# value is read out of `args[0]` into a Rust local, GC may run
# between that read and the `heap.alloc` for the new Array, and
# nothing pins the local across the gap — see issue #90, site #8.
# This fixture exercises the path with heap-bearing Value
# flavours (String, user instance) so STRESS_GC flushes any
# rooting hole on the wrap.
#
# We deliberately avoid Hash / Range / Struct in this fixture:
# CRuby coerces those via `to_a` (so `Array({k:1})` →
# `[[:k, 1]]`), but the rubyrs subset wraps them like any other
# non-Array value (`[{k:1}]`). Those cases live in
# tests/subset/* as documented divergences; here we want a
# fixture whose oracle is CRuby. String and Object are coerced
# identically by both.

# String — heap-bearing; the original site #8 candidate. 1000
# iterations is enough to take many trips through GC at
# STRESS_GC=1 (every alloc triggers a full mark+sweep), so any
# rooting hole on `args[0]` will reuse the freed slot and
# either ICE or produce garbage output here.
1000.times do
  a = Array("hello")
  raise "string wrap broken" unless a == ["hello"]
end
puts "string ok"

# User instance — Object subclass, hits HeapObj::Object on the
# wrap. We compare by length + class rather than `==` because
# default Ruby equality on Object is identity.
class Holder
  def initialize(v); @v = v; end
  attr_reader :v
end
1000.times do
  a = Array(Holder.new(42))
  raise "instance wrap broken" unless a.length == 1 && a[0].v == 42
end
puts "instance ok"

# The nil / Array cases don't take the maybe_gc + alloc path,
# but include them so the fixture documents the whole Array()
# contract end to end.
puts Array(nil).inspect       # []
puts Array([1, 2, 3]).inspect # [1, 2, 3]
