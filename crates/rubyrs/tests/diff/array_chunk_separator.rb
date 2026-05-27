# `Array#chunk` separator semantics — `nil` keys drop the element
# AND end the current group, so equal keys on either side of a
# separator land in *different* groups (CRuby parity).
#
# Pre-fix rubyrs treated `nil` only as a "skip this element"
# sentinel without resetting the same-as-last tracking, so the
# example below collapsed both `1`s into one group.
# Surfaced by Copilot review on PR #187.

# Both 1s are separated by a nil-key element → must be 2 groups.
puts [1, 2, 1].chunk { |x| x == 2 ? nil : x }.to_a.inspect
# → [[1, [1]], [1, [1]]]

# Two separators in a row collapse: 3,4 split, both 4s stay together,
# then nil splits again into a single 5.
puts [3, 4, 4, 0, 5].chunk { |x| x == 0 ? nil : x }.to_a.inspect
# → [[3, [3]], [4, [4, 4]], [5, [5]]]

# Separator at the start / end is a no-op (no group to terminate).
puts [0, 1, 1, 0].chunk { |x| x == 0 ? nil : x }.to_a.inspect
# → [[1, [1, 1]]]

# Sanity: without a separator, equal consecutive keys do merge.
puts [1, 1, 1].chunk { |x| x }.to_a.inspect
# → [[1, [1, 1, 1]]]
