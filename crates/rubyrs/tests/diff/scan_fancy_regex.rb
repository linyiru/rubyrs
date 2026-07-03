# String#scan is now dual-engine: patterns needing the fancy-regex engine
# (lookahead / lookbehind / backreferences) work, not just the native
# regex crate. liquid's `if`/`unless` parser scans with a lookahead
# pattern (`ExpressionsAndOperators`), so this unblocks Liquid conditions.
p "foo bar baz".scan(/\w+(?=\s)/)              # ["foo","bar"] (lookahead)
p "a1b2c3".scan(/(?<=[a-z])\d/)                # ["1","2","3"] (lookbehind)
p "hello".scan(/(.)\1/)                        # [["l"]] (backreference group)
p "aXbXcX".scan(/(\w)(?=X)/)                   # [["a"],["b"],["c"]]
p "no match".scan(/(?<=z)x/)                   # []
p $~                                           # nil after no-match scan
# native (non-fancy) patterns unchanged
p "a,b,c".scan(/\w/)                           # ["a","b","c"]
p "k1=v1 k2=v2".scan(/(\w+)=(\w+)/)            # [["k1","v1"],["k2","v2"]]
p "".scan(/x/)                                 # []
p "aaa".scan(/a/)                              # ["a","a","a"]
# optional unmatched group → nil in the result tuple
p "abc".scan(/(a)(z)?/)                        # [["a",nil]]

# RuboCop's MatchRange mixin drives these fancy-only patterns through
# block-form String#scan. With captures, the block receives the capture
# Array, and $~/$1/offsets reflect the active iteration.
rubocop_spaces = /(?:[\S&&[^\\]](?:\\ )*)( {2,})(?=\S)/
p "foo  bar baz".scan(rubocop_spaces)          # [["  "]]
p [$~[0], $1, $~.offset(1)]                    # final no-block match
hits = []
"foo  bar baz".scan(rubocop_spaces) do |groups|
  hits << [groups, $~[0], $1, $~.offset(1)]
end
p hits
p [$~[0], $1, $~.offset(1)]

trailing_spaces = /(?<!\\)( +)\z/
trailing = []
"a   ".scan(trailing_spaces) do |groups|
  trailing << [groups, $~[0], $1]
end
p trailing
p [$~[0], $1]

plain_fancy = /(?<=foo)bar/
plain_hits = []
"foobar fooqux foobar".scan(plain_fancy) do |hit|
  plain_hits << [hit, $~[0]]
end
p plain_hits                                # block receives String without captures
p [$~[0], $1]
"zzz".scan(plain_fancy) { |hit| p hit }
p $~                                       # nil after no-match block scan
