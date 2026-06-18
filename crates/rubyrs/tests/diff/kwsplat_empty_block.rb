# foo(**empty, &blk) — an empty/nil keyword-splat passes ZERO args even
# when a block is also forwarded (previously the empty hash leaked in as
# a positional → spurious arity errors). Tilt's fixed-locals render does
# `compiled_method.bind_call(scope, **locals, &block)`.
def takes_none; "ok"; end
blk = nil
h = {}
p takes_none(**h, &blk)
p takes_none(**{}, &blk)
b2 = proc { 1 }
p takes_none(**h, &b2)

# non-empty kwsplat + block still binds as kwargs
def kw(a:, b:); "#{a}/#{b}"; end
opts = {a: 1, b: 2}
p kw(**opts, &blk)

# explicit empty-hash positional + block is NOT dropped (stays positional)
def one(x); x.class; end
p one({}, &blk)

# bind_call(scope, **{}, &blk) on a zero-arg method (the Tilt shape)
module TOP; end
TOP.class_eval { def m; "m"; end }
um = TOP.instance_method(:m)
p um.bind_call(Object.new, **{}, &blk)
p um.bind_call(Object.new, **h, &b2)
