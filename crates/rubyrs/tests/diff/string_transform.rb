# sub — first occurrence only
puts "hello".sub("l", "L")
puts "hello hello".sub("hello", "bye")
puts "abc".sub("z", "Z")          # no match — original
puts "abc".sub("", "X")           # empty pattern inserts at start
puts "".sub("a", "b")             # empty receiver — unchanged

# gsub — all occurrences
puts "hello".gsub("l", "L")
puts "hello hello".gsub("hello", "bye")
puts "aaaa".gsub("a", "bc")
puts "abc".gsub("z", "Z")         # no match
puts "abc".gsub("", "_")          # empty pattern wraps each char
puts "".gsub("a", "b")

# Chained
puts "foo bar".sub("o", "0").gsub(" ", "_")
puts "/usr/local/bin".gsub("/", "-")

# tr — char-by-char translation
puts "hello".tr("el", "ip")       # eli -> ip; e->i, l->p
puts "abcdef".tr("ace", "ACE")
puts "hello".tr("aeiou", "*")     # stretch — all vowels -> '*'
puts "hello".tr("lo", "")         # empty `to` deletes
puts "ABC".tr("ABC", "xy")        # short `to` — extra A,B,C map to LAST of `to` ('y')
puts "abc".tr("", "")             # noop
puts "hello".tr("ll", "rr")       # l->r, l->r

# Realistic — path / id sanitisation idioms
puts "Hello, World!".gsub(",", "").gsub(" ", "_")
puts "v1.2.3".tr(".", "_")
puts "/path/with/slashes".tr("/", "-")

# Inside a method with default args (composition sanity)
def slugify(s, sep = "-")
  s.downcase.gsub(" ", sep).gsub(",", "").gsub("!", "")
end
puts slugify("Hello, World!")
puts slugify("Quick Brown Fox", "_")

# respond_to? on the new methods
puts "x".respond_to?(:sub)
puts "x".respond_to?(:gsub)
puts "x".respond_to?(:tr)
