# String#scan is now dual-engine: patterns needing the fancy-regex engine
# (lookahead / lookbehind / backreferences) work, not just the native
# regex crate. liquid's `if`/`unless` parser scans with a lookahead
# pattern (`ExpressionsAndOperators`), so this unblocks Liquid conditions.
p "foo bar baz".scan(/\w+(?=\s)/)              # ["foo","bar"] (lookahead)
p "a1b2c3".scan(/(?<=[a-z])\d/)                # ["1","2","3"] (lookbehind)
p "hello".scan(/(.)\1/)                        # [["l"]] (backreference group)
p "aXbXcX".scan(/(\w)(?=X)/)                   # [["a"],["b"],["c"]]
p "no match".scan(/(?<=z)x/)                   # []
# native (non-fancy) patterns unchanged
p "a,b,c".scan(/\w/)                           # ["a","b","c"]
p "k1=v1 k2=v2".scan(/(\w+)=(\w+)/)            # [["k1","v1"],["k2","v2"]]
p "".scan(/x/)                                 # []
p "aaa".scan(/a/)                              # ["a","a","a"]
# optional unmatched group → nil in the result tuple
p "abc".scan(/(a)(z)?/)                        # [["a",nil]]
