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

# Block-form still works alongside (sanity check —
# `iter_array_filter` handles the block path). `one?` with a
# block isn't implemented yet (no-block lands in array.rs;
# block form would need a new IterMode in iter_array_filter).
p [1, 2, 3].any? { |x| x > 2 }
p [1, 2, 3].all? { |x| x > 0 }
p [1, 2, 3].none? { |x| x > 5 }
