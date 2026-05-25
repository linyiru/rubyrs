# Hash-mutation microbench. Stresses the Hash#[]= write path
# (collision probe + insert) and Hash#[] read path (key compare).
# Reads-after-writes also exercise the ruby_eq Hash key
# comparison.

h = {}
i = 0
while i < 200_000
  key = (i & 1023)         # 1024-bucket key space → repeated overwrites
  h[key] = i
  i = i + 1
end

# Now read everything back and digest.
total = 0
k = 0
while k < 1024
  total = total + (h[k] || 0)
  k = k + 1
end
puts total
puts h.size
