# `\G` anchor in the match-AT-POSITION paths (`String#match`/`match?`/
# `Regexp#match?` with an offset). `preprocess_regex_pattern` strips `\G`
# so the linear engine can compile, but the match-at-pos paths re-honour it
# by anchoring the match to EXACTLY the search position (the start of the
# sliced tail), instead of letting the rest of the pattern scan forward.
#
# Driver: rubocop-ast's `Token#space_after?` / `#space_before?` are
# `source.match(/\G\s/, pos)` — without this, `Layout/SpaceInsideParens`
# (and other token-spacing cops) false-positive on every `(`/`)`.

s = "ab cd"   # index: a0 b1 (space)2 c3 d4

# --- String#match(/\G\s/, pos): whitespace EXACTLY at pos, no forward scan
p s.match(/\G\s/, 0).nil?           # true  (char@0 = 'a')
p s.match(/\G\s/, 1).nil?           # true  (char@1 = 'b')
p s.match(/\G\s/, 2).nil?           # false (char@2 = ' ')
p s.match(/\G\s/, 2)[0]             # " "
p s.match(/\G\s/, 3).nil?           # true  (char@3 = 'c')

# --- String#match? with pos honours \G too
p s.match?(/\G\s/, 0)               # false
p s.match?(/\G\s/, 2)               # true
p s.match?(/\G\s/, 3)               # false

# --- Regexp#match? with pos
p(/\G\s/.match?(s, 2))              # true
p(/\G\s/.match?(s, 1))              # false

# --- the rubocop space_after?/space_before? shape, verbatim
def space_after?(src, end_pos)  = !src.match(/\G\s/, end_pos).nil?
def space_before?(src, begin_pos)
  pos = begin_pos.zero? ? begin_pos : begin_pos - 1
  !src.match(/\G\s/, pos).nil?
end
tight = "f(x)"     # f0 (1 x2 )3 — no inner spaces
p space_after?(tight, 2)            # false (char@2 = 'x', right after '(')
p space_before?(tight, 3)           # false (char@2 = 'x', right before ')')
paren = "f( x )"   # f0 (1 (space)2 x3 (space)4 )5
p space_after?(paren, 2)            # true  (char@2 = ' ', right after '(')
p space_before?(paren, 5)           # true  (char@4 = ' ', right before ')')

# --- NON-\G patterns keep forward-search-from-pos (unchanged)
p s.match(/c/, 0)[0]                # "c"  (forward scan finds it at 3)
p s.match(/\w/, 2)[0]              # "c"  (skips the space, finds 'c')

# --- \G at pos 0 still equals \A-at-0 (the common single-match case)
p("hello".match(/\Ghello/)[0])     # "hello"
p("x hello".match(/\Ghello/).nil?) # true  (\G requires pos 0; 'x' there)

# --- \G with captures, at pos
"k=v".match(/\G(\w+)=(\w+)/, 0)
p $1                                # "k"
p $2                                # "v"
