# String#=~ returns a CHARACTER index (consistent with String#index),
# not the regex engine's byte offset. The two diverge on multibyte
# input; ASCII is unaffected (byte == char).
p("café x" =~ /x/)         # 5
p("café x".index("x"))     # 5 (consistent)
p("héllo wörld" =~ /w/)    # 6
p("αβγδ" =~ /γ/)           # 2
p("abc" =~ /b/)            # 1
p("café" =~ /z/)           # nil

# `$~` captures still resolve after the char-indexed return.
"a→b→c" =~ /(→)(.)/
p $~[1]                    # "→"
p $~[2]                    # "b"
p $~.pre_match             # "a"
p $~.post_match            # "→c"
