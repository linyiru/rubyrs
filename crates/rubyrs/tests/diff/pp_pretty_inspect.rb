# `require "pp"` installs Object#pretty_inspect + the PP module
# (Kernel#pp is a native builtin). faraday's logging formatter calls
# `body.pretty_inspect`. Tier-1: pretty_inspect is single-line #inspect
# + a trailing newline (matches CRuby byte-for-byte on short values).
p require("pp")                       # true
print [1, 2, 3].pretty_inspect        # [1, 2, 3]\n
print({a: 1, b: 2}.pretty_inspect)    # {a: 1, b: 2}\n
print "hi".pretty_inspect             # "hi"\n
print 42.pretty_inspect               # 42\n
print nil.pretty_inspect              # nil\n
print :sym.pretty_inspect             # :sym\n
print({}.pretty_inspect)              # {}\n
p PP.pp([1, 2], "".dup)               # "[1, 2]\n"
p PP.singleline_pp([1, 2], "".dup)    # "[1, 2]"
p require("pp")                       # false (already loaded)
