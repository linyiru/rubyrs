# Array#bsearch_index — binary search returning the INDEX (find-minimum
# for a boolean block, find-any for an Integer block). Driver:
# parser/source/buffer.rb's `line_begins.bsearch_index { |b| pos < b }`,
# gated on `Array.method_defined?(:bsearch_index)`.
p Array.method_defined?(:bsearch_index)

a = [0, 4, 7, 10, 12]
p a.bsearch_index { |x| x >= 7 }        # 2
p a.bsearch_index { |x| x >= 0 }        # 0
p a.bsearch_index { |x| x >= 100 }      # nil
p [].bsearch_index { |x| x >= 1 }       # nil
# find-any mode
p [1, 2, 3, 4].bsearch_index { |x| 3 - x }   # 2
p [1, 2, 3, 4].bsearch_index { |x| 9 - x }   # nil
# the parser's line-lookup shape
line_begins = [0, 6, 12, 20]
idx = line_begins.bsearch_index { |b| 9 < b }
p(idx.nil? ? line_begins.size - 1 : idx - 1)  # 1
p a.bsearch_index.class                  # Enumerator
