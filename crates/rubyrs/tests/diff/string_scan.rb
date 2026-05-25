# String#scan — regex matching that returns either an Array of
# match strings (no capture groups) or an Array of capture-group
# Arrays (with groups). Block form yields each match and returns
# the receiver.

# Non-block, no captures.
puts "hello world foo".scan(/\w+/).inspect      # ["hello","world","foo"]
puts "abc123def456".scan(/\d+/).inspect         # ["123","456"]
puts "nothing here".scan(/x/).inspect           # []

# Non-block, with captures.
puts "a1b2c3".scan(/([a-z])(\d)/).inspect       # [["a","1"],["b","2"],["c","3"]]
puts "k=v;a=b".scan(/(\w+)=(\w+)/).inspect      # [["k","v"],["a","b"]]

# Literal string pattern (no captures concept).
puts "h-e-l-l-o".scan("-").inspect              # ["-","-","-","-"]

# Block form, no captures — yields each match.
result = []
"foo bar baz".scan(/\w+/) { |m| result << m.upcase }
puts result.inspect                             # ["FOO","BAR","BAZ"]

# Block returns the receiver.
ret = "a b c".scan(/\w/) { |m| m }
puts ret                                        # "a b c"

# Block form, with captures — yields each capture-group Array.
pairs = []
"k=1, j=2, m=3".scan(/(\w+)=(\d+)/) { |g| pairs << "#{g[0]}=>#{g[1]}" }
puts pairs.inspect                              # ["k=>1","j=>2","m=>3"]

# Block form with string pattern.
sep_count = 0
"a-b-c-d".scan("-") { |_| sep_count += 1 }
puts sep_count                                  # 3
