# each_slice / each_cons now work on any Enumerable (Enumerator, Integer
# iterators, Range#each) via the preamble Enumerable module — Array/Hash
# keep their native forms (these don't shadow them).
p [1,2,3,4,5].each.each_slice(2).to_a
p [1,2,3,4,5].each.each_cons(2).to_a
r = []; [1,2,3,4,5].each.each_slice(2) { |s| r << s }; p r
r = []; [1,2,3,4].each.each_cons(3) { |c| r << c }; p r
p [1,2].each.each_slice(5).to_a          # slice bigger than input
p [].each.each_slice(2).to_a             # empty
p [1,2].each.each_cons(5).to_a           # window bigger than input → []
p 5.times.each_slice(2).to_a             # Integer enumerator chaining
p 10.times.each_slice(3).map(&:sum)
p (1..6).each.each_cons(2).to_a          # Range#each enumerator
p (1..6).each.each_slice(2).map(&:last)
begin; [1,2].each.each_slice(0).to_a; rescue => e; p [e.class, e.message]; end
begin; [1,2].each.each_cons(-1).to_a; rescue => e; p [e.class, e.message]; end
# native Array/Range forms unaffected
p [1,2,3,4].each_slice(2).to_a
p (1..4).each_cons(2).to_a
