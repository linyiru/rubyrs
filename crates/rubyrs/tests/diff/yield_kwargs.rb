# `yield(a: 1, b: 2)` / `yield a: 1` — the `k: v` keyword sugar reaches
# Prism as a trailing KeywordHashNode (same as a call site). CRuby
# yields it as a single trailing Hash; the block's `|h|` / `|**o|`
# binding extracts it. Previously the KeywordHashNode tripped the
# unsupported-node path and the whole file failed to compile.

def m1; yield(a: 1, b: 2); end
m1 { |**o| p o }

def m2; yield(a: 1); end
m2 { |h| p h }

# no parentheses
def m3; yield a: 3, b: 4; end
m3 { |**o| p o }

# positional arg + trailing kwargs
def m4; yield 1, x: 5; end
m4 { |n, **o| p [n, o] }

# dynamic values
def m5(k); yield(k: k, two: k * 2); end
m5(7) { |**o| p o }

# explicit hash arg followed by trailing kwargs
def m6; yield({ a: 1 }, b: 2); end
m6 { |h, **o| p [h, o] }

# double-splat
def m7(opts); yield(**opts); end
m7({ a: 1, b: 2 }) { |**o| p o }

# literal key + double-splat merge
def m8(opts); yield(x: 0, **opts); end
m8({ a: 1 }) { |**o| p o }

# empty arg list still yields nothing
def m9; yield(); end
m9 { |*a| p a }
