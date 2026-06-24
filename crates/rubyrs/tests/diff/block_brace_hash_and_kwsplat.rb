# Block calls: a trailing BRACE hash is positional in Ruby 3 (only `k: v`
# / `**h` are kwargs). rubyrs's Op::CallBlock used to peel a brace hash as
# kwargs (`m({k:1}, &b)` → "given 0"); and the splat+kwsplat+block shape
# `n(*a, **{}, &b)` dropped the positional too. Both fixed (CallBlock now
# treats the trailing hash positional; Op::ApplyCallKwBlock carries the
# kwsplat separately). Surfaced by generic `def d(*a, **o, &b)=n(*a,**o,&b)`
# delegators (ActiveSupport / Rails forwarding).
bl = proc { "B" }

# --- direct block call: brace hash stays positional ---
def m(a, **o, &b) = [a, o, (b ? b.call : nil)]
p m({ k: 1 }, &bl)         # [{k:1}, {}, "B"]
p m("x", &bl)              # ["x", {}, "B"]
h = { z: 9 }
p m(h, &bl)                # [{z:9}, {}, "B"]
# bare kwargs with a required positional → ArgumentError on both (compare msg)
r = begin; m(k: 1, &bl); :ok; rescue ArgumentError => e; e.message; end
p r

# --- splat + kwsplat + block (Op::ApplyCallKwBlock) ---
def n(a, **o, &b) = [a, o, (b ? b.call : nil)]
empty = {}
p n(*[{ q: 1 }], **empty, &bl)      # [{q:1}, {}, "B"]
p n(*[{ q: 1 }], **{ x: 5 }, &bl)   # [{q:1}, {x:5}, "B"]
def deleg(*args, **opts, &blk) = n(*args, **opts, &blk)
p deleg({ id: 7 }, &bl)             # [{id:7}, {}, "B"]
p deleg({ id: 7 }, tag: :z, &bl)    # [{id:7}, {tag: :z}, "B"]
