# Array#assoc / #rassoc — find a sub-Array whose first (assoc)
# or second (rassoc) element equals the needle. Returns the
# first match; nil if none. Non-Array elements in the receiver
# are silently skipped, matching CRuby.

pairs = [[:a, 1], [:b, 2], [:c, 3]]

# assoc: first sub-Array with matching [0].
puts pairs.assoc(:b).inspect          # [:b, 2]
puts pairs.assoc(:a).inspect          # [:a, 1]
puts pairs.assoc(:z).inspect          # nil

# rassoc: first sub-Array with matching [1].
puts pairs.rassoc(2).inspect          # [:b, 2]
puts pairs.rassoc(99).inspect         # nil

# Skip non-Array elements without raising.
puts [1, 2, [3, 4], "skip", [5, 6]].assoc(3).inspect   # [3, 4]
puts [1, 2, [3, 4], "skip", [5, 6]].rassoc(6).inspect  # [5, 6]

# Empty receiver.
puts [].assoc(:a).inspect             # nil
puts [].rassoc(1).inspect             # nil

# Mixed types in keys.
puts [["k", 1], [:k, 2]].assoc("k").inspect   # ["k", 1]
puts [["k", 1], [:k, 2]].assoc(:k).inspect    # [:k, 2]

# Realistic: Array-of-pairs as poor-man's ordered map.
servers = [["web", 80], ["api", 443], ["db", 5432]]
puts servers.assoc("api").inspect     # ["api", 443]
puts servers.rassoc(80).inspect       # ["web", 80]
