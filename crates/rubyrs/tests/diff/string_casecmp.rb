# String#casecmp / #casecmp? — ASCII case-insensitive compare. casecmp
# returns -1/0/1, casecmp? a Bool; both nil for a non-String arg. (rouge's
# batchfile lexer uses casecmp.)
p "Hello".casecmp("hello")        # 0
p "a".casecmp("b")                # -1
p "b".casecmp("a")                # 1
p "Hello".casecmp("World")        # -1
p "ABCDEF".casecmp("abcdef")      # 0
p "abc".casecmp("abcd")           # -1 (length tiebreak)
p "abc".casecmp?("ABC")           # true
p "a".casecmp?("b")               # false
p "Hello".casecmp?("hello")       # true
p "x".casecmp(1)                  # nil
p "x".casecmp?(:sym)              # nil
p "".casecmp("")                  # 0
