# GC-pressure microbench. Allocates many short-lived Arrays and
# Hashes per iteration; the lifetime is one block invocation so
# nearly every allocation becomes unreachable before the next
# collection cycle. Designed to stress mark/sweep frequency and
# the maybe_gc heuristic rather than the dispatch loop.

def churn(n)
  total = 0
  i = 0
  while i < n
    pair = [i, i + 1]
    hash = { a: pair, b: i * 2 }
    total = total + hash[:b]
    triple = [pair, hash, "tag_#{i}"]
    total = total + triple.length
    i = i + 1
  end
  total
end

puts churn(200_000)
