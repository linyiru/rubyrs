# String#squeeze — collapse runs of consecutive identical chars.
# With a char-set arg, only chars in the set are squeezed.
# Range syntax (`"a-z"`) and ^-negation work via the shared
# `parse_tr_set` (same as `tr` / `count`). This fixture pins
# the literal-char baseline; range/negation coverage lives in
# spec/ruby/string_squeeze_spec.rb.

# No-arg form.
puts "aaabbbccc".squeeze               # abc
puts "Mississippi".squeeze             # Misisipi
puts "  hello  ".squeeze               # " hello "
puts "".squeeze                        # ""
puts "x".squeeze                       # x
puts "aaa".squeeze                     # a

# Char-set arg: only listed chars squeeze.
puts "aabbcc".squeeze("a")             # abbcc
puts "aabbcc".squeeze("b")             # aabcc
puts "aabbcc".squeeze("ab")            # abcc
puts "Mississippi".squeeze("ips")      # Misisipi (i, p, s collapse; M doesn't)
puts "aabbcc".squeeze("xyz")           # aabbcc (no set member, no change)

# Set with a char not in the string.
puts "hello".squeeze("xl")             # helo (only l squeezed)
