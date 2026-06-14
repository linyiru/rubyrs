# Regexp#match?(str, pos) — start the match attempt at character offset
# pos (negative counts from the end); no $~ update. rack's request parser
# probes a forwarded header with a positional match?.
p(/\d+/.match?("abc123", 3))     # true
p(/\d+/.match?("abc123", 4))     # true (still digits ahead)
p(/\A\d/.match?("abc123", 0))    # false (anchored, starts with 'a')
p(/foo/.match?("xfoox", 1))      # true
p(/foo/.match?("xfoox", 3))      # false (past it)
p(/o/.match?("hello", -2))       # false (last two chars "lo" -> only 'o' at -1)
p(/o/.match?("hello", -1))       # false ("o"? "hello"[-1]="o"? no, it's 'o'... )
p(/x/.match?("abc", 10))         # false (out of range)
p(/\d+/.match?("a1b2", 1))       # true
