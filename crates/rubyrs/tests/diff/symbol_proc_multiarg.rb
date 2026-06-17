# `&:sym` forwards ALL yielded args to the method: `recv.sym(*rest)`.
# Multi-arg methods (binary operators via reduce/inject) now work, and
# it does NOT auto-splat a single Array (so `&:first` over pairs keeps
# the pair as the receiver).

# Multi-arg: the operator gets its operand.
p [1, 2, 3, 4].reduce(&:+)
p [1, 2, 3, 4].inject(&:*)
p [10, 3, 8].reduce(&:-)

# No auto-splat: the pair is the receiver, not split into args.
p [[1, 2], [3, 4]].map(&:first)
p [[1, 2], [3, 4]].map(&:last)
p [[1, 2], [3, 4]].map(&:sum)
p [[1, 2, 3], [4, 5]].map(&:length)

# Common zero-extra-arg methods still work.
p [1, 2, 3].map(&:to_s)
p ["a", "b"].map(&:upcase)
p [1, -2, 3].map(&:abs)
p [1, 2, 3, 4].select(&:even?)
p [1.4, 2.6].map(&:round)
p [:x, :y].map(&:to_proc).map { |pr| pr.call("hi") } rescue p [:x, :y].map(&:to_s)

# Explicit .to_proc with extra args.
plus = :+.to_proc
p plus.call(2, 3)
