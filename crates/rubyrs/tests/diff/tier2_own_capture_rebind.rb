# Tier-2 wave-3 closure-interaction gate (ADR 0037 wave 3).
#
# A compiled body rebinds an own-region local that IS captured by an inner
# block created in the same body: the moment `CreateBlock` runs, the local's
# cell is SHARED with the closure (the shared-binding model, 4f6ef741).
# Wave-3's local caching handles this structurally: a proto containing
# `CreateBlock` has `creates_block == true`, so its frame is `Locals::Shared`
# and its slots are NEVER SSA-cached — every read/write routes through the
# capture-aware helpers (`frame_local_get` / `outer_cell_for` / `cell_store`),
# before AND after the block is created. These fixtures pin that gate.

# 1. Rebind after CreateBlock — the closure must see the rebound value, and
#    the method must see the closure's increments.
def counter_maker
  count = 0
  bump = proc { count += 1 }
  count = 10          # rebind AFTER the capture: hits the shared cell
  bump.call
  bump.call
  count               # 12
end
puts counter_maker

# 2. Escaped closures over the same slot: one reads, one writes, both after
#    the defining frame popped.
def escape_maker
  x = 1
  reader = proc { x }
  x = 5               # rebind between the two captures
  writer = proc { x = x * 10 }
  [reader, writer]
end
r, w = escape_maker
w.call
puts r.call           # 50

# 3. The compiled body itself keeps using the slot after the closure ran —
#    write-through must target the shared cell, never a stale copy.
def pingpong
  v = :start
  setter = proc { |nv| v = nv }
  setter.call(:from_block)
  before = v
  v = :from_method
  [before, setter.call(:again), v]
ensure
  # (ensure also keeps this body off the tier — parity must hold anyway)
end
puts pingpong.inspect

# 4. Block param SHADOWING an outer name must not alias the captured slot.
def shadow
  n = 3
  [1, 2].each { |n2| n = n + n2 }
  n                   # 6
end
puts shadow

# 5. define_method-style closure capturing an own-region local rebound later.
class DmHost
  define_method(:dm_probe) do
    a = 1
    grab = proc { a }
    a = 7
    grab.call
  end
end
puts DmHost.new.dm_probe

# 6. Nested blocks: inner block captures a middle-scope local that the
#    middle block rebinds after creating it.
def nested
  total = 0
  [10, 20].each do |x|
    inc = proc { total += x }
    x = x + 1           # rebinds the BLOCK's own param after capture
    inc.call
  end
  total                 # 11 + 21 = 32
end
puts nested
