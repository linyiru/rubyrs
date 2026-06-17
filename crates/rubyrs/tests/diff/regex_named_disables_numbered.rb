# When a regex contains ANY named capture group, Ruby/Onigmo demotes
# every UNNAMED `(…)` group to non-capturing — only named groups are
# numbered. So size/captures/$1/[] all reflect named groups only.

m = "llo w".match(/(l+)o (w)(?<r>w?)/)
p m.size            # whole + the one NAMED group
p m.captures        # named values only
p m.named_captures
p m[1]              # numbered index → the named group at position 1
p m[:r]
p m.to_a

# $1 / $~ after a mixed match: the unnamed (a) is non-capturing.
"abc" =~ /(a)(?<x>b)c/
p $1
p $~[:x]
p $~.captures

# Unnamed-only patterns are unaffected (still numbered).
m2 = "ab".match(/(a)(b)/)
p m2.captures
p m2.size

# Lookbehind/lookahead are NOT named groups, must not be demoted.
p "foobar".match(/(?<=foo)(?<g>bar)/)[:g]
p "ab".match(/a(?=b)(?<h>b)/)[:h]

# Escaped paren and a paren inside a char class are literals.
p "a(b)c".match(/a\((?<z>b)\)c/)[:z]
p "a(c".match(/(?<w>a[(]c)/)[:w]

# Offsets/begin refer to the named groups' renumbered positions.
md = "xxllo wq".match(/(l+)o (?<k>w)/)
p md.begin(1)
p md.offset(:k)
