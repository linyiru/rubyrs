# String#[]= — companion to `array_assign_slice.rb`. Pins:
#   * Range LHS (new — `s[range] = repl`)
#   * Float index coerce on 2-arg / 3-arg forms (new)
#   * Existing 2-arg Int + 3-arg Int,Int regression guard
#   * Boundary / negative-wrap edges
#
# Out of scope (separate gap): `s["substr"] = "repl"` substring
# replace + IndexError on miss. Same follow-up note as the
# read-side fixture; the lookup helper would live in
# `vm/string.rs` either way.

# --- Existing single-Int (regression-guard) ---
s = "hello"; s[0] = "H"; p s
s = "hello"; s[-1] = "O"; p s

# --- Existing 3-arg Int,Int,Str ---
s = "hello"; s[1, 2] = "XX"; p s
s = "hello"; s[1, 2] = "YYY"; p s    # expand
s = "hello"; s[1, 2] = ""; p s       # contract
s = "hello"; s[5, 0] = "!"; p s      # boundary append

# --- Float coerce on []= ---
s = "hello"; s[2.5] = "X"; p s       # truncates to 2
s = "hello"; s[0, 2.5] = "AB"; p s
s = "hello"; s[0.5, 2] = "CD"; p s

# --- Range LHS (new) ---
s = "hello"; s[1..2] = "XX"; p s
s = "hello"; s[1..2] = "YYY"; p s    # expand
s = "hello"; s[1..2] = ""; p s       # delete
s = "hello"; s[1...3] = "ZZ"; p s    # exclusive
s = "hello"; s[1...3] = "WWWW"; p s  # exclusive expand
s = "hello"; s[-2..-1] = "OO"; p s   # negative wrap
s = "hello"; s[0..0] = "HE"; p s
s = "hello"; s[..1] = "BYE"; p s     # beginless
s = "hello"; s[3..] = "OOO"; p s     # endless
s = "hello"; s[3...] = "OOO"; p s    # endless exclusive — same shape
s = "hello"; s[..-1] = "WORLD"; p s  # full-replace via beginless-to-last

# --- Real-shape idioms ---
s = "hello world"; s[0..4] = "HOWDY"; p s
s = "x"; s[0..-1] = "expanded"; p s
