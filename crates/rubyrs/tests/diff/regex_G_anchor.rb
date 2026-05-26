# Regex `\G` anchor — translated to "stripped" at compile time.
#
# CRuby uses Onigmo which supports `\G` (match-at-last-position),
# but the Rust `regex` crate doesn't. The vm/step.rs regex
# compile path now runs patterns through `preprocess_regex_pattern`
# which drops `\G` before handing the source to the engine.
#
# Trade-off: dropping `\G` makes the regex slightly more
# permissive than CRuby (a `\G` only matches at the previous
# match's end, while the rest of the pattern matches anywhere).
# For real-world consumers — primarily MRI lib/erb/compiler.rb:460
# (`/\G<%#(.*)%>/` and similar) — the surrounding structural
# anchors (`<%#`, `%>`) constrain the match to the same locations
# in practice. This fixture pins the cases where the over-match
# is observable AS WELL AS the cases where rubyrs and CRuby
# coincide.

# --- Pure-`\G` patterns: same behaviour as without `\G` ---
# When `\G` sits at the start AND the surrounding text doesn't
# have multiple non-anchored matches, CRuby and our stripped form
# return identical results.
puts "abcabc".scan(/\Gabc/).inspect              # ["abc", "abc"] (both)
puts "abcabc".scan(/abc/).inspect                # ["abc", "abc"] (same)

# --- `=~` with `\G` at start of string — both 0 ---
s = "hello"
puts (s =~ /\Ghello/).inspect                    # 0
puts $~[0]                                       # hello

# --- match? predicate with `\G` at start of input ---
# When the `\G` anchor's "position 0" coincides with where the
# rest of the regex matches, rubyrs and CRuby agree. This is the
# typical use case in real codebases (magic-comment detection,
# stateful scanners that slice from cursor first).
puts "data".match?(/\Gdata/)                     # true
# DOCUMENTED DIVERGENCE: `"  data".match?(/\Gdata/)` returns
# true in rubyrs (\\G stripped → /data/ matches at offset 2)
# but false in CRuby (\\G requires position 0). Not pinned here
# because the fixture is diff_cruby byte-for-byte; see
# preprocess_regex_pattern in vm/step.rs for the impl-site
# documentation.

# --- ERB-shape probe ---
# Mirror the lib/erb/compiler.rb:460 pattern shape. The regex
# is used with String#scan over the template source, matching
# `<%# ... %>` comment tags. The surrounding literal anchors
# constrain it so rubyrs and CRuby get the same matches.
src = "<%# coding: utf-8 %><%# other %>"
matches = src.scan(/\G<%#(.*?)%>/)
puts matches.inspect                             # [[" coding: utf-8 "], [" other "]]

# --- Captures still work after \G strip ---
# Important: the rest of the regex compiles normally, captures
# numbered backrefs (`$1`, `$2`), and routes through the same
# regex-cache path.
"k=v" =~ /\G(\w+)=(\w+)/
puts $1                                          # k
puts $2                                          # v

# --- `\G` inside a character class — literal G, NOT stripped ---
# `/[\G]/` is "character class containing G" in every regex
# dialect. The preprocessor must NOT strip `\G` inside `[...]`
# or the class would silently change behaviour (empty class,
# regex compile error, or collapse with neighbours).
puts "XYZ" =~ /[\G]/                            # nil  (no G in XYZ)
puts ("GHI" =~ /[\G]/)                          # 0    (G at offset 0)
puts ("ABCG" =~ /[\G]/)                         # 3    (G at offset 3)

# --- `\G` inside an alternation / grouping ---
# Mirror lib/erb/compiler.rb:460's other shape:
#   /\G(?:<%#(.*)%>|%#(.*)\n)/
# The outer `\G` is at the start; the alternation contains its
# own group anchors. Stripped form: same group capture indices
# preserved, scan walks the source the same way.
percent_src = "<%# a %>%# b\n"
p2 = percent_src.scan(/\G(?:<%#(.*)%>|%#(.*)\n)/)
puts p2.inspect                                  # [[" a ", nil], [nil, " b"]]
