# chunk_while / slice_when (Tier-1 returns the materialized Array of
# chunks; CRuby returns an Enumerator — consumers iterate either way,
# so compare via to_a on the CRuby side... both sides print Arrays
# here because p on our Array == p on their enum.to_a equivalent.
p [1, 2, 4, 5, 7].chunk_while { |a, b| b - a == 1 }.to_a
p %w[a a b a].chunk_while { |a, b| a == b }.to_a
p [].chunk_while { |a, b| a == b }.to_a
p [1, 2, 4, 5, 7].slice_when { |a, b| b - a > 1 }.to_a
p [3].slice_when { |a, b| true }.to_a
p [1, 2, 4, 5, 7].each_cons(2).chunk_while { |a, b| a == b }.to_a.length
