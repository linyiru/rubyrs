# `Array#any?` / `#all?` / `#none?` / `#one?` no-block forms.
# CRuby's contract: tests element truthiness, no block needed.
# rubyrs already had the block-form (in `iter_array_filter`);
# this commit fixes the no-block path, motivated by
# rack-protection's `parts.any?` predicate inside
# `PathTraversal#cleanup`.

# any? — true iff at least one element is truthy.
p [].any?
p [nil].any?
p [false].any?
p [nil, false].any?
p [1].any?
p [nil, 1, false].any?
p [false, "x"].any?

# all? — true iff every element is truthy.
p [].all?           # vacuously true
p [1, 2].all?
p [1, nil, 2].all?
p [1, false, 2].all?
p [true].all?

# none? — true iff no element is truthy.
p [].none?          # vacuously true
p [nil, false].none?
p [nil, 1].none?
p [1].none?

# one? — true iff exactly one element is truthy.
p [].one?           # zero matches
p [1].one?
p [1, 2].one?       # two matches
p [nil, 1, false].one?
p [nil, false].one?  # zero matches

# Block-form coverage. `one?` block-form now also lands —
# `iter_array_filter` gained an IterMode::One arm that
# short-circuits on the SECOND truthy match. Range#one? gets
# the same treatment via iter_range_filter.
p [1, 2, 3].any? { |x| x > 2 }
p [1, 2, 3].all? { |x| x > 0 }
p [1, 2, 3].none? { |x| x > 5 }
p [1, 2, 3].one? { |x| x == 2 }       # exactly one match
p [1, 2, 3].one? { |x| x > 0 }        # three matches → false
p [].one? { |x| true }                # zero matches → false
p [1, 2, 3].one? { |x| x > 5 }        # zero matches → false
p (1..5).one? { |x| x == 3 }          # Range#one? — exactly one
p (1..5).one? { |x| x > 2 }           # multiple → false
