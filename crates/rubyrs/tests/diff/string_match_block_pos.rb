# String#match full signature: optional char-index `pos` second arg
# and the block form.

# --- block form ---
# Match → block is called with the MatchData, returns the block value.
p("hello".match(/l(l)o/) { |m| "got #{m[1]}" })
# No match → block NOT called, returns nil.
p("hello".match(/zzz/) { |m| "should not run" })
# Block value propagates (any type).
p("abc".match(/b/) { |m| 42 })
# Coerced String pattern + block.
p("a1b2".match("(\\d)") { |m| m[1].to_i * 10 })

# $~ is set by the block form too.
"hello".match(/l+/) { |_| }
p $~[0]

# --- pos (2nd arg) form ---
p "hello hello".match(/hello/, 1)   # finds the SECOND hello
p "hello".match(/l/, 3)             # the second l
p "abc".match(/a/, 1)               # past 'a' → nil
p "hello".match(/l/, -2)            # negative pos counts from end
p "abc".match(/a/, 99)              # pos out of range → nil

# pos affects $~ offsets (pre_match is relative to the whole string).
m = "xxhello".match(/hello/, 2)
p m.pre_match
p m.post_match

# pos + block together.
p("aXbXc".match(/X/, 2) { |mm| mm.pre_match })
