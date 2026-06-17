# `yield(*v, **h)` — a yield whose args mix a positional splat with a
# keyword double-splat. The splat sends it down the YieldSplat assembly
# path, which translated the trailing `**h` KeywordHashNode with the
# generic `tr` (→ "unsupported node") instead of tr_kwhash. Surfaced by
# the pp gem's `kwsplat ? yield(*v, **kwsplat) : yield(*v)` (pp.rb:277).
def m1
  v = [1]; h = {x: 9}
  yield(*v, **h)
end
m1 { |n, x:| p [n, x] }            # [1, 9]

# splat + literal keywords
def m2
  a = [10, 20]
  yield(*a, k: 3)
end
m2 { |x, y, k:| p [x, y, k] }      # [10, 20, 3]

# double-splat only (no positional splat) — already worked, kept as guard
def m3
  h = {a: 1, b: 2}
  yield(**h)
end
m3 { |a:, b:| p [a, b] }           # [1, 2]

# splat + double-splat merged with explicit kwarg
def m4
  rest = [:p, :q]; opts = {m: 1}
  yield(*rest, n: 2, **opts)
end
m4 { |a, b, n:, m:| p [a, b, n, m] }  # [:p, :q, 2, 1]

# a plain block (no kw params) receives the trailing hash positionally
def m5
  v = [1]; h = {z: 8}
  yield(*v, **h)
end
m5 { |a, b| p [a, b] }             # [1, {z: 8}]
