# Flip-flop `a..b` / `a...b` in boolean context: off until `a` is truthy,
# then on until `b` is truthy. 2-dot checks `b` on the turn-on eval; 3-dot
# defers it. State persists across an enclosing loop's iterations.

# 2-dot
(1..8).each { |i| print i if (i == 2)..(i == 4) }
puts

# 3-dot defers the end check
(1..8).each { |i| print i if (i == 2)...(i == 4) }
puts

# start == end: 2-dot turns on and off in the same eval
(1..5).each { |i| print i if (i == 3)..(i == 3) }
puts

# start == end: 3-dot stays on (end check deferred)
(1..6).each { |i| print i if (i == 3)...(i == 3) }
puts

# canonical line-range filter via a while loop
lines = ["pre", "BEGIN", "a", "b", "END", "post"]
i = 0
kept = []
while i < lines.length
  kept << lines[i] if (lines[i] == "BEGIN")..(lines[i] == "END")
  i += 1
end
p kept

# never turns on
(1..3).each { |i| print i if (i == 9)..(i == 10) }
puts "(none)"
