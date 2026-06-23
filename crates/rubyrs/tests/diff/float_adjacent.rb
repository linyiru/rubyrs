# Float#next_float / #prev_float — the adjacent representable doubles
# (IEEE-754 nextUp / nextDown). Byte-stable against CRuby: the
# results are exact doubles, and the saturation / NaN edges match.

p 1.0.next_float          # 1.0000000000000002
p 1.0.prev_float          # 0.9999999999999999
p 0.0.next_float          # 5.0e-324
p 0.0.prev_float          # -5.0e-324
p (-0.0).next_float       # 5.0e-324
p (-0.0).prev_float       # -5.0e-324
p 1.0.next_float.prev_float  # 1.0  (round-trips)

p Float::INFINITY.next_float == Float::INFINITY        # true
p Float::INFINITY.prev_float == Float::MAX             # true
p (-Float::INFINITY).next_float == (-Float::MAX)       # true
p (-Float::INFINITY).prev_float == (-Float::INFINITY)  # true
p (0.0 / 0.0).next_float.nan?   # true
p (0.0 / 0.0).prev_float.nan?   # true
