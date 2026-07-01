# A `\k<name>` backreference to a DUPLICATED capture-group name (the
# same name defined on both sides of an alternation). Onigmo resolves it
# to whichever branch matched; the Rust engines reject the ambiguous
# `\k<name>` (fancy-regex) or backrefs entirely (regex), so rubyrs
# rewrites the ambiguous in-pattern backref to a numeric one. Exact
# shape lifted from RuboCop's Lint/DuplicateMethods#humanize_scope.
re = /(?:(?<name>.*)::)#<Class:\k<name>>|#<Class:(?<name>.*)>(?:::)?/

# Branch B (second `(?<name>…)`) — the case real RuboCop exercises:
# input is always a plain name or "#<Class:X>". Replacement backref by
# name resolves to the participating group.
p "#<Class:Foo>".sub(re, '\k<name>.')
p "Foo::#<Class:Bar>".sub(re, '\k<name>.')
# No match — the pattern must still compile and leave the string alone.
p "PlainScope".sub(re, '\k<name>.')

# Name-based capture access resolves the collapsed duplicate name.
p re.match("#<Class:Zap>")&.named_captures
p re.match("Ns::#<Class:Ns>")&.named_captures

# In-pattern backref to a duplicated name matches correctly in BOTH
# alternation branches (this is match-side, not replacement-side).
re2 = /(?<w>\w+)=\k<w>|(?<w>\d+)/
p "ab=ab".match(re2)&.to_a
p "42".match(re2)&.to_a
p "xy=zz".match(re2)&.to_a

# NOTE: a `\k<name>` in the REPLACEMENT of an alternation's FIRST branch
# (e.g. "Foo::#<Class:Foo>".sub(re, '\k<name>.')) resolves to the
# last-defined dup group, not the participating one — a known limitation
# that RuboCop never hits (its humanize_scope input is only branch B).
