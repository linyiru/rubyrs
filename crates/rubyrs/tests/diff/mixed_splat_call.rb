# Mixed splat in calls: foo(*arr, x, **opts) — positional splat
# interleaved with literal args, plus double-splat kwargs.

def f(*args, **opts)
  p args
  p opts
end

arr = [1, 2, 3]
opts = { a: 10, b: 20 }

# Splat at start.
f(*arr)
f(*arr, 99)

# Splat in middle.
f(0, *arr, 99)

# Splat + kwargs.
f(*arr, c: 100)
f(*arr, **opts)
f(*arr, c: 100, **opts)
f(0, *arr, 99, c: 100, **opts)

# Two splats.
a = [1, 2]
b = [3, 4]
f(*a, *b)
f(*a, 100, *b)

# Array literal splat (the building block).
p [*arr]
p [0, *arr, 99]
p [0, *arr, *[10, 20]]

# Hash literal with double-splat.
h = { x: 1, y: 2 }
p({ z: 3, **h })
p({ z: 3, **h, w: 4 })
