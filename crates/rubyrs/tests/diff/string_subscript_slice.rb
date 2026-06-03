# String#[] (slice) — companion to `array_subscript_slice.rb`.
# String already had 1-arg Int + 2-arg Int+Int + 1-arg Range
# forms; this fixture pins:
#   * Float index coercion (Float→Int truncate, matching CRuby)
#   * Boundary / negative-wrap / endless-range edge cases that
#     parallel the Array slice surface
#
# Out of scope (separate gap): `s["substr"]` substring lookup —
# returns the matched substring or nil. Tracked as a follow-up
# alongside `s["substr"] = "x"` (substring replace via
# IndexError-on-miss).

s = "hello"

# --- 1-arg Int (regression-guard) ---
p s[0]
p s[4]
p s[-1]
p s[-5]
p s[5]      # past-end → nil
p s[-6]     # under-start → nil

# --- 1-arg Float coerce (post fix) ---
p s[2.5]    # → "l"   (truncated to 2)
p s[-1.9]   # → "o"   (truncated to -1)
p s[0.0]    # → "h"

# --- 2-arg Int, Int ---
p s[0, 2]
p s[1, 3]
p s[0, 100]   # length clamps
p s[-2, 2]
p s[5, 0]     # boundary → ""
p s[5, 2]     # boundary → ""
p s[6, 2]     # past end → nil
p s[0, -1]    # negative length → nil

# --- 2-arg Float coerce ---
p s[0, 2.5]   # → "he"
p s[0.5, 2]   # → "he"
p s[1.9, 2.5] # both coerce

# --- Range (inclusive + exclusive) ---
p s[0..2]
p s[1..-1]
p s[1...3]
p s[1...-1]
p s[2..]      # endless
p s[..2]      # beginless
p s[2...]     # endless exclusive — same as endless inclusive
p s[5..]      # boundary → ""
p s[6..]      # past end → nil
p s[3..1]     # begin > end → ""
