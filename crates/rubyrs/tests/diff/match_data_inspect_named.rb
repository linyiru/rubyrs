# MatchData#inspect: when the pattern has any named capture, CRuby
# lists ONLY the named groups (name:value, in pattern order) and
# suppresses the numbered list. Otherwise it shows numbered groups.

# All named.
p "ab".match(/(?<key>\w)(?<val>\w)/)

# All positional.
p "ab".match(/(\w)(\w)/)

# Mixed: only the named group is shown (the unnamed group 1 is hidden).
p "ab".match(/(\w)(?<k>\w)/)

# Named + an unnamed optional that didn't participate: only named shown.
p "a".match(/(?<x>a)(y)?/)

# A named group that didn't participate serialises as name:nil.
p "x".match(/(?<a>x)|(?<b>y)/)

# No groups at all: no trailing list.
p "x".match(/x/)

# Non-participating positional group serialises as N:nil.
p "ac".match(/(a)(x)?(c)/)

# $~ inside a scan block inspects the same way.
"A=1\nB=2".scan(/(?<key>\w)=(?<val>\w)/) { p $~ }
