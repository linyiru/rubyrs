# Symbol-to-proc — `&:method_name` desugars to `{ |x| x.method_name }`.
# The canonical CRuby ergonomics for "apply this method to each".

# Basic map.
p [1, 2, 3].map(&:to_s)
p ["a", "bb", "ccc"].map(&:length)
p [1.5, 2.5, 3.5].map(&:to_i)
p [:foo, :bar, :baz].map(&:to_s)

# Math-style.
p [-1, -2, -3, 4, 5].map(&:abs)
p [1, 2, 3, 4, 5].map(&:even?)
p [1, 2, 3, 4, 5].map(&:odd?)

# sort_by(&:length).
p ["banana", "fig", "apple"].sort_by(&:length)
p [[1, 2, 3], [1], [1, 2]].sort_by(&:length)

# min_by / max_by.
p ["banana", "fig", "apple"].min_by(&:length)
p ["banana", "fig", "apple"].max_by(&:length)

# select / reject.
p [1, 2, 3, 4, 5].select(&:even?)
p [1, 2, 3, 4, 5].reject(&:odd?)

# any? / all? / none? — predicate-style block.
p [1, 3, 5].all?(&:odd?)
p [1, 3, 4].all?(&:odd?)
p [1, 3, 4].any?(&:even?)
p [1, 3, 5].none?(&:even?)

# find / detect.
p [1, 2, 3, 4].find(&:even?)

# Chains.
p [1, -2, 3, -4].map(&:abs).select(&:even?)

# group_by.
p [1, 2, 3, 4, 5].group_by(&:even?)

# String-on-array.
p ["Hello", "World", "!"].map(&:upcase)
p ["  hi  ", "\nbye\n"].map(&:strip)
p ["abc", "DEF"].map(&:reverse)

# Inside a method.
def lengths(strs)
  strs.map(&:length)
end

p lengths(["a", "bcd", "ef"])

# inject / reduce don't accept &:symbol because the block needs
# 2 args; CRuby errors on that. Don't test it here.

# Inspect chain works because Inspect is on every type.
p [1, "x", :s, true, nil].map(&:inspect)

# Combined with sort + reverse for "by length descending".
p ["aaa", "b", "cc"].sort_by(&:length).reverse
