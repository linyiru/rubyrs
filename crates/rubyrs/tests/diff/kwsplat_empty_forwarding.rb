# An empty `**kwsplat` contributes nothing: `f(x, **{})` passes just `x`,
# so a preceding positional hash stays POSITIONAL (Ruby 3 semantics).
# rubyrs used to peel the positional hash into kwargs when the kwsplat was
# empty, leaving the required positional unbound ("given 0, expected 1").
# Surfaced by ActiveSupport's `create_message(value, **options)` chain.
def m(value, **options) = [value, options]
empty = {}
nonempty = { x: 9 }
p m({ a: 1 }, **empty)        # [{a:1}, {}]
p m({ a: 1 }, **nonempty)     # [{a:1}, {x:9}]
p m({ a: 1 }, **{})           # [{a:1}, {}]  (literal empty splat)
p m("s", **empty)             # ["s", {}]
p m(42, k: 1, **empty)        # [42, {k:1}]
h = { a: 1 }
p m(h, **empty)               # [{a:1}, {}]

# Forwarding chain (the AS shape): outer forwards value + **options to
# inner; with empty options the value must survive as positional.
def inner(value, **options) = [value, options]
def outer(value, **options) = inner(value, **options)
p outer({ id: 7 })            # [{id:7}, {}]
p outer({ id: 7 }, tag: :z)   # [{id:7}, {tag: :z}]
