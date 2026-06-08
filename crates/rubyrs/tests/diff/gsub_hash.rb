# String#gsub / #sub with a Hash replacement: each match is looked up (as
# a String) in the hash and replaced with the mapped value, or "" when
# absent. This is rouge's HTML-escape path
# (`gsub(ESCAPE_REGEX, TABLE_FOR_ESCAPE_HTML)`).
p "a<b>c".gsub(/[<>]/, "<" => "LT", ">" => "GT")        # "aLTbGTc"
p "hello".gsub(/l/, "l" => "L")                          # "heLLo"
p "abc".gsub(/x/, "x" => "Y")                            # "abc" (no match)
p "cat".gsub(/[aeiou]/, "a" => "4", "e" => "3")          # "c4t"
p "aeiou".gsub(/[aeiou]/, "a" => "4", "e" => "3")        # "43" + missing -> "" => "43"
# HTML escape table (the rouge shape)
esc = { "&" => "&amp;", "<" => "&lt;", ">" => "&gt;" }
p "a & b < c > d".gsub(/[&<>]/, esc)                     # "a &amp; b &lt; c &gt; d"
# sub: first match only
p "a<b>c".sub(/[<>]/, "<" => "LT", ">" => "GT")          # "aLTb>c"
p "xxx".sub(/x/, "x" => "Y")                             # "Yxx"
# string pattern + hash
p "aaa".gsub("a", "a" => "X")                            # "XXX"
p "aaa".sub("a", "a" => "X")                             # "Xaa"
p "hello world".gsub("o", "o" => "0")                    # "hell0 w0rld"
# value coercion via to_s (non-string value)
p "n1n2".gsub(/[12]/, "1" => 1, "2" => 2)                # "n1n2" (1.to_s, 2.to_s)
# integer values
p "ab".gsub(/[ab]/, "a" => :A, "b" => :B)                # "AB" (symbol to_s)
