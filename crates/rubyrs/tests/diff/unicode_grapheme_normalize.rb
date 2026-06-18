# UAX#29 grapheme segmentation + UCD normalization for non-ASCII
# (unicode-segmentation / unicode-normalization crates).
p "abc".each_grapheme_cluster.to_a
p "café".each_grapheme_cluster.to_a
p "é".each_grapheme_cluster.to_a
p "é".grapheme_clusters
p "👨‍👩‍👧".grapheme_clusters
p "".each_grapheme_cluster.to_a
p "hi".each_grapheme_cluster.class
n = 0; "café".each_grapheme_cluster { |g| n += 1 }; p n
p "é".unicode_normalize(:nfc).bytes
p "é".unicode_normalize(:nfd).bytes
p "ﬁ".unicode_normalize(:nfkc)
p "²".unicode_normalize(:nfkc)
p "abc".unicode_normalize
p "café".unicode_normalize == "café"
p "café".unicode_normalized?(:nfc)
p "é".unicode_normalized?(:nfc)
p "abc".grapheme_clusters
begin; "x".unicode_normalize(:bad); rescue ArgumentError => e; p e.message; end
