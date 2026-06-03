# Array#[] subscript surface — single Integer index (already
# worked) + two-arg `start, length` slice + Range slice with all
# the edge cases CRuby exposes. Discovered as a missing surface
# while implementing AS-lite Tier D-narrow Duration#inspect's
# Oxford-comma formatter (commit f53bc4ee) where `pieces[0..-2]`
# / `pieces[0, n - 1]` would have been the natural shape but
# rubyrs's Array#[] only supported the single-Int form. Closes
# the gap so the next consumer can write the natural Ruby.

a = [1, 2, 3, 4, 5]

# --- Single-Integer (pre-existing, regression-guard) ---
p a[0]
p a[4]
p a[-1]
p a[-5]
p a[5]      # past-end → nil
p a[-6]     # under-start → nil

# --- Two-arg `start, length` slice ---
p a[0, 2]
p a[1, 2]
p a[3, 2]
p a[0, 100]    # length clamps to receiver
p a[-2, 2]     # negative start wraps from end
p a[-2, 100]   # negative start + over-length
p a[5, 2]      # start == len → [] (NOT nil — boundary rule)
p a[6, 2]      # start > len → nil
p a[0, -1]     # negative length → nil
p a[0, 0]      # zero length → []

# --- Inclusive Range (`a..b`) ---
p a[0..2]
p a[1..3]
p a[1..-1]
p a[1..-2]
p a[3..6]    # end clamps at len - 1
p a[5..6]    # begin == len → []
p a[6..7]    # begin > len → nil
p a[2..1]    # begin > end (after wrap) → []

# --- Exclusive Range (`a...b`) ---
p a[0...2]
p a[1...3]
p a[1...-1]
p a[1...-2]
p a[3...6]
p a[5...6]
p a[6...7]

# --- Beginless / Endless Ranges (Ruby 2.6+ / 2.7+) ---
p a[2..]      # endless: from idx 2 to end
p a[2...]     # endless exclusive — same as endless inclusive
p a[..2]      # beginless: from idx 0 inclusive to 2
p a[...2]     # beginless exclusive: 0 to 1
p a[-2..]     # negative-start endless
p a[..-1]     # beginless to last (== full array)

# --- Composition with existing methods ---
p a[0..2].length
p a[1, 2].map { |x| x * 10 }
p a[2..].first
p a[..3].last(2)

# --- Empty array ---
e = []
p e[0]
p e[0, 5]
p e[0..2]
p e[1..]
