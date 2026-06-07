# Block-form gsub/sub works on patterns that need the fancy-regex
# engine (lookahead / backrefs), and $~ (incl. named groups) is live
# inside the block.
p "a1b2c3".gsub(/\d(?=[a-z]|$)/) { |m| "[#{m}]" }
p "x12y34".gsub(/(?<n>\d+)/) { "<#{$~[:n]}>" }
p "foo bar baz".sub(/(?<=foo )\w+/) { |w| w.upcase }
p "aXbXc".gsub(/X/) { $~.pre_match.length.to_s }
