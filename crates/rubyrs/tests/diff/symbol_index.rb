# Symbol#[] delegates to to_s[...] (CRuby) — index, range, regex, substring
# slicing of the symbol's string form. Surfaced by ostruct's method_missing
# (`mid[/.*(?==\z)/m]` to peel a `name=` setter).
p :foobar[0]            # "f"
p :foobar[1, 3]         # "oob"
p :foobar[0..2]         # "foo"
p :foobar[-1]           # "r"
p :foobar[/o+/]         # "oo"
p :foobar[/z/]          # nil
p :"foo="[/.*(?==\z)/m] # "foo" (the ostruct setter-name peel)
p :foobar[10]           # nil (out of range)
# (String-arg substring form `sym["sub"]` delegates to String#[], which has
# its own separate gap — not exercised here.)
