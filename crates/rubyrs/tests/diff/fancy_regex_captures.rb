# `=~` and `String#match` capture extraction on patterns that
# require the fancy-regex engine. The linear `regex` crate
# rejects `\Z` (and lookaround / backrefs), so those patterns
# fall back to fancy-regex; pre-fix `=~` / `match` on a fancy
# pattern raised
#   RuntimeError: regex op '=~' is not yet supported on patterns
#   requiring the fancy-regex engine
# Now both ops extract captures engine-agnostically.
#
# Discovery: P3 Sinatra spike -- Mustermann wraps every route in
# `/\A(?-mix:...)\Z/`, and the `\Z` anchor forces fancy, so route
# matching (`pattern.match(path).captures`) depends on this.

# Shape 1: `\Z` anchor (forces fancy) with a named capture --
# the Mustermann route shape.
re = /\Ausers\/(?<id>[^\/]+)\Z/
puts "s1_pos=#{("users/42" =~ re).inspect}"
m = "users/42".match(re)
puts "s1_named=#{m[:id]}"
puts "s1_pos_cap=#{m[1]}"
puts "s1_whole=#{m[0]}"
puts "s1_miss=#{("nope" =~ re).inspect}"

# Shape 2: the literal Mustermann compiled shape.
mre = /\A(?-mix:(.*))\Z/
puts "s2_pos=#{("hello" =~ mre).inspect}"
puts "s2_cap=#{"hello".match(mre)[1]}"

# Shape 3: lookahead (genuinely fancy) with a trailing capture.
la = /foo(?=bar)(.)/
lm = "foobarX".match(la)
puts "s3=#{lm.nil? ? 'nil' : lm[1]}"

# Shape 4: backreference (fancy) -- matches a doubled word.
br = /\A(\w+) \1\Z/
bm = "the the".match(br)
puts "s4_match=#{!bm.nil?}"
puts "s4_cap=#{bm[1]}" if bm
puts "s4_miss=#{"the cat".match(br).inspect}"

# Shape 5: $~ / $1 globals populate from a fancy =~.
# (NB: `$~.pre_match` from a bare `=~` is a separate pre-existing
# gap -- the `=~` path's `$~` doesn't carry pre/post-match
# context, same on linear patterns -- so it isn't asserted here.)
"order/99" =~ /\Aorder\/(\d+)\Z/
puts "s5_g1=#{$1}"
puts "s5_whole=#{$~[0]}"

# Shape 6: named_captures hash on a fancy match.
nm = "2026-06-06".match(/\A(?<y>\d+)-(?<m>\d+)-(?<d>\d+)\Z/)
puts "s6=#{nm.named_captures.inspect}"

# Shape 7: a failed fancy match clears the globals.
"x" =~ /\A(\d)\Z/
puts "s7_after_miss=#{$~.inspect}"

# Shape 8: non-participating group in a fancy alternation -> nil.
am = "cat".match(/\A(dog)|(cat)\Z/)
puts "s8_g1=#{am[1].inspect}"
puts "s8_g2=#{am[2].inspect}"
