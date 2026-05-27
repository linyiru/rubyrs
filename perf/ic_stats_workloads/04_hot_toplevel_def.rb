# Hot toplevel user `def` — implicit-self dispatch through
# `lookup_toplevel_method_cache_hit`. Expected: toplevel_hits
# ~ 0.999. Cross-check for PR #170's fast-path counter fix.
N = 10_000
def helper
  42
end
total = 0
i = 0
while i < N
  total += helper
  i += 1
end
puts total
