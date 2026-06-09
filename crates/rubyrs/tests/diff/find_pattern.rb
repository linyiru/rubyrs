# Find patterns `[*pre, m…, *post]` — match a consecutive run anywhere in
# the array; pre/post bind the slices before/after the FIRST such run.

# bind pre/post around a literal middle
case [1, 2, 3, 4, 5]
in [*pre, 3, *post] then p [pre, post]
end

# anonymous edges, bind the middle pair (first run)
case [1, 2, 3, 4, 5]
in [*, x, y, *] then p [x, y]
end

# first matching element by class
case ["a", "b", "c"]
in [*, String => s, *] then p s
end

# pre + typed middle + post, guard on the FIRST matched element
case [10, 20, 30, 40]
in [*head, Integer => n, *tail] if n < 100 then p [head, n, tail]
end

# guard fails on the first match (no backtracking) -> NoMatchingPatternError
begin
  case [10, 20, 30]
  in [*, Integer => n, *] if n > 15 then p n
  end
rescue NoMatchingPatternError
  p :guard_failed
end

# no run matches -> else
case [1, 2, 3]
in [*, 99, *] then p :found
else p :not_found
end

# whole array is the run (empty pre/post)
case [7, 8]
in [*pre, 7, 8, *post] then p [pre, post]
end

# Const-tagged find pattern
case [0, 5, 0]
in Array[*, 5, *] then p :has_five
end
