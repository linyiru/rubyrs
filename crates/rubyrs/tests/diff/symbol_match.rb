# Symbol#match / #match? delegate to the symbol's string form (CRuby:
# `sym.to_s.match(...)`). Surfaced by ostruct/oj guarding names with
# `name.match(/.../)`.
p :foobar.match(/o+/)[0]            # "oo"
p :foobar.match(/(o)(o)/).captures  # ["o", "o"]
p :foobar.match(/zzz/)              # nil
p :foobar.match("ba")[0]            # "ba" (string pattern → Regexp)
p :foobar.match?(/bar/)             # true
p :foobar.match?(/zzz/)             # false
p :Camel.match?(/[A-Z]/)            # true
