# any?/all?/none?/one? with a PATTERN argument (CRuby 2.5+):
# tests `pattern === element` instead of truthiness. minitest's
# `failures.any? UnexpectedError` is the motivating caller.
p [1, "a", :s].any?(String)
p [1, 2].any?(String)
p [1, 2].none?(String)
p [1, "a", "b"].one?(String)
p [1, "a"].one?(String)
p [1, 5, 9].any?(3..6)
p [2, 4].all?(1..5)
p ["ab", "cd"].all?(/[a-d]+/)
p({ a: 1 }.any?(Array))
p (1..4).any?(2..3)
