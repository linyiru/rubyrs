# Array#slice! in its index / start+length / range forms, and the
# zero-arg Array#unshift / #prepend (the `a.unshift(*empty)` shape).
def show(label, a, r)
  p [label, r, a]
end

a = [1, 2, 3, 4, 5]; show(:idx, a, a.slice!(1))
a = [1, 2, 3, 4, 5]; show(:neg_idx, a, a.slice!(-1))
a = [1, 2, 3, 4, 5]; show(:idx_oob, a, a.slice!(9))
a = [1, 2, 3, 4, 5]; show(:start_len, a, a.slice!(1, 2))
a = [1, 2, 3, 4, 5]; show(:start_len_long, a, a.slice!(3, 10))
a = [1, 2, 3, 4, 5]; show(:start_at_end, a, a.slice!(5, 1))
a = [1, 2, 3, 4, 5]; show(:start_past_end, a, a.slice!(6, 1))
a = [1, 2, 3, 4, 5]; show(:neg_len, a, a.slice!(1, -1))
a = [1, 2, 3, 4, 5]; show(:neg_start, a, a.slice!(-2, 2))
a = [1, 2, 3, 4, 5]; show(:range, a, a.slice!(1..2))
a = [1, 2, 3, 4, 5]; show(:range_excl, a, a.slice!(1...2))
a = [1, 2, 3, 4, 5]; show(:range_neg, a, a.slice!(2..-1))
a = [1, 2, 3, 4, 5]; show(:range_endless, a, a.slice!(3..))
a = [1, 2, 3, 4, 5]; show(:range_beginless, a, a.slice!(..1))
a = [1, 2, 3, 4, 5]; show(:range_empty, a, a.slice!(3..1))
a = [1, 2, 3, 4, 5]; show(:range_at_end, a, a.slice!(5..))
a = [1, 2, 3, 4, 5]; show(:range_oob, a, a.slice!(7..9))

# tsort's shape: truncate a stack back to a saved length.
stack = [:a, :b, :c, :d]
saved = 2
p stack.slice!(saved..-1)
p stack

empty = []
b = [1, 2]
p b.unshift(*empty).equal?(b)
p b.prepend.equal?(b)
p b.unshift(0)
