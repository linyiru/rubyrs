# gsub/sub with a STRING pattern and a block: the pattern matches
# literally (metacharacters are escaped), each match is yielded, and
# the block's result is the replacement. The no-block gsub("str") also
# returns an Enumerator that drives this path.
p "hello".gsub("l") { |m| m.upcase }
p "hello".sub("l") { |m| m.upcase }
p "h1e2l3".gsub("1") { |m| "[#{m}]" }

# String pattern is LITERAL — "." matches a dot, not any char.
p "a.b.c".gsub(".") { "_" }
p "a.b.c".gsub(".", "_")
p "axbxc".gsub(".") { "_" }

# Enumerator forms over a string pattern.
p "hello".gsub("l").to_a
p "hello".gsub("l").count
p "hello".gsub("l").with_index { |m, i| i.to_s }

# Regex form still works alongside.
p "hello".gsub(/l/) { |m| m.upcase }
