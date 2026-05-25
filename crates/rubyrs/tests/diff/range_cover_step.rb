# Range#cover?(Range) — true iff the other range is fully
# within self. Empty sub-ranges (begin >= end with excl, or
# begin > end inclusive) do NOT cover — matches CRuby.
# Range#step(n) block form — yields each step value.

# cover? with Int (existing) still works.
puts (1..10).cover?(5)         # true
puts (1..10).cover?(15)        # false

# cover?(Range) — basic.
puts (1..10).cover?(3..7)      # true
puts (1..10).cover?(1..10)     # true  (self)
puts (1..10).cover?(0..5)      # false (low spills)
puts (1..10).cover?(5..15)     # false (high spills)
puts (1..10).cover?(5..5)      # true  (1-element)

# Empty sub-range → false.
puts (1..10).cover?(8...8)     # false
puts (1..10).cover?(8..7)      # false
puts (1..10).cover?(0...0)     # false

# Exclusive self.
puts (1...10).cover?(1..9)     # true  (excl, 9 < 10)
puts (1...10).cover?(1..10)    # false (10 not in excl 10)
puts (1...10).cover?(1...10)   # true

# Range#step block form. Returns receiver.
collected = []
(1..10).step(2) { |x| collected << x }
puts collected.inspect         # [1, 3, 5, 7, 9]

collected2 = []
(0..20).step(5) { |x| collected2 << x }
puts collected2.inspect        # [0, 5, 10, 15, 20]

# step is non-mutating; receiver returned.
r = (1..5)
ret = r.step(2) { |_| }
puts ret == r                  # true

# break short-circuits.
first_two = []
(1..100).step(10) { |x| first_two << x; break if first_two.length == 2 }
puts first_two.inspect         # [1, 11]
