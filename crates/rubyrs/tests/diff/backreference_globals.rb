# BackReferenceReadNode — regex special globals.
#
# CRuby's "side-channel" globals updated by every successful
# regex match: `$&` (whole match), `$+` (last non-nil group),
# `` $` `` (pre-match), `$'` (post-match), `$~` (MatchData).
# Numbered backrefs (`$1`, `$2`, ...) are a separate Prism node
# class (`NumberedReferenceReadNode`) and were already pinned
# by `regex_match` / `regex_basics`. This fixture pins the
# BackReferenceReadNode family.
#
# Motivating real-world use: tilt vendors MRI's erb.rb, whose
# `ERB::Compiler#detect_magic_comment` reads `$+` immediately
# after a `String#[/regex/]` match (lib/erb/compiler.rb:457).

# --- No match yet: every backref reads as nil ---
# Before any regex runs, last_match is None — even though
# Prism parses these as BackReferenceReadNode, they all
# resolve to nil. Pin this so a future refactor doesn't
# accidentally raise instead.
puts $&.inspect                                 # nil
puts $+.inspect                                 # nil
puts $`.inspect                                 # nil
puts $'.inspect                                 # nil
puts $~.inspect                                 # nil

# --- Whole match (`$&`) ---
"hello world" =~ /wor(l)d/
puts $&                                         # world

# --- Pre-match (`` $` ``) and post-match (`$'`) ---
# Slices of the original input, on either side of the match.
puts $`                                         # "hello " (with trailing space)
puts $'                                         # "" (empty — match ended at EOS)

"prefix-MIDDLE-suffix" =~ /MID(D)LE/
puts $`                                         # prefix-
puts $'                                         # -suffix

# --- Last non-nil capture (`$+`) ---
# The right-most group that actually participated. If the last
# group is `nil`, walk leftward until a non-nil capture is
# found.
"abc" =~ /(a)(b)(c)/
puts $+                                         # c
"abc" =~ /(a)(b)(z)?/
puts $+                                         # b (3rd group didn't match)

# --- ERB-shape probe ---
# Mirror lib/erb/compiler.rb:457 in spirit — after a regex
# match against a magic-comment string, `$+` is the captured
# encoding name. (CRuby's actual code uses `String#[/regex/]`
# which also populates the backref globals; we use `=~` here
# because it goes through the same `Vm::last_match` path and
# pinning two distinct entry points would just duplicate.)
comment = "-*- encoding: utf-8 -*-"
comment =~ /-\*-\s*([^\s].*?)\s*-\*-$/
puts $+                                         # encoding: utf-8

# --- A failed match clears the side channel ---
# CRuby: `=~`/`match`/etc. all wipe `$~`/$1..$N AND
# `$&`/`$+`/`` $` ``/`$'` on miss. Same single source of
# truth (`Vm::last_match`), so they all flip to nil together.
"no digits here" =~ /\d+/
puts $&.inspect                                 # nil
puts $+.inspect                                 # nil
puts $`.inspect                                 # nil
puts $'.inspect                                 # nil

# --- After-success, after-failure ordering ---
# Confirm that a fresh match overwrites stale state.
"a-b" =~ /(a)-(b)/
"no match" =~ /\d+/
puts $&.inspect                                 # nil — most-recent-match wins

# --- `$+` with no parenthesised groups ---
# CRuby: returns nil. The regex matched but there are no
# capture groups to be "last non-nil" of.
"hi" =~ /hi/
puts $+.inspect                                 # nil
puts $&                                         # hi (whole match still set)
