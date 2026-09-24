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

# A huge length / range end clamps to "through the end"; one whose
# span end overflows a 64-bit long raises (CRuby's check).
a = [1, 2, 3, 4, 5]; show(:len_huge, a, a.slice!(1, 4_611_686_018_427_387_904))
a = [1, 2, 3, 4, 5]; show(:range_end_huge, a, a.slice!(1..4_611_686_018_427_387_904))
[[1, 9_223_372_036_854_775_807], [1..9_223_372_036_854_775_807]].each do |args|
  begin
    [1, 2, 3].slice!(*args)
    p :no_error
  rescue ArgumentError => e
    p [e.class, e.message]
  end
end

# Unsupported shapes still belong to slice!: arity / conversion errors,
# not NoMethodError. Float indices truncate.
[[], [0, 1, 2], ["x"], [0, "x"], [nil]].each do |args|
  begin
    [1, 2, 3].slice!(*args)
    p :no_error
  rescue ArgumentError, TypeError => e
    p [e.class, e.message]
  end
end
p [1, 2, 3].slice!(1.9)
p [1, 2, 3].slice!(0.0, 2.5)
