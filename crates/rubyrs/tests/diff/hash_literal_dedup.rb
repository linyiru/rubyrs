# Hash literal dedup: last-write-wins, strict eql? (no Int<->Float),
# first-occurrence position, mixed key types — must match CRuby.
p({a: 1, b: 2, c: 3})
p({a: 1, a: 2})                 # {a: 2}
p({1 => :a, 1 => :b})           # {1 => :b}
p({1.0 => :a, 1 => :b}.size)    # 2 (strict eql?)
p({"x" => 1, "x" => 2})         # {"x" => 2}
p({})
p({k: 1})
h = {x: 1, y: 2, x: 3}
p h                             # {x: 3, y: 2}
p h.keys                        # [:x, :y]
p({a: 1, b: 2, c: 3, d: 4, e: 5, f: 6}.size)
n = 10
p({sum: n + 5, prod: n * 2})
