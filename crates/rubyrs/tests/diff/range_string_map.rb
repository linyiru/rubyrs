# Range#map/collect over String endpoints (str_succ walk, same
# corpus as Range#to_a) — minitest's SystemStackError compressor
# maps ("a".."z").
p ("a".."e").map { |s| s.upcase }
p ("a"..."d").map { |s| s }
p ("aa".."ac").collect { |s| "| #{s}" }
p ("y".."ab").map { |s| s }
p (1..4).map { |i| i * 2 }
p ("b".."a").map { |s| s }
