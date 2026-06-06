# String#intern is a CRuby alias of String#to_sym. Discovery: P3
# Jekyll spike — kramdown's `utils/configurable.rb` calls
# `name.intern`.
p "foo".intern
p "foo".intern == :foo
p "foo".intern.equal?(:foo)        # interned to the same Symbol
p "with space".intern
p "".intern
p "to_sym agrees: #{("bar".intern == "bar".to_sym)}"
p :baz.to_s.intern == :baz         # round-trip Symbol->String->Symbol
