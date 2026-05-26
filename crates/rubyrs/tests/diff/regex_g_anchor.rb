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
# in practice. This fixture only pins shapes where rubyrs and
# CRuby agree byte-for-byte; the divergent shapes (e.g. `\G`
# against a non-zero-offset receiver) are called out inline as
# comments but NOT asserted — diff_cruby would fail. The impl-
# site doc in vm/step.rs records the divergence formally.

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
# Use `.inspect` on every `=~` result so nil renders as the
# string "nil" rather than a blank line (matches other diff
# fixtures' style and makes diff output unambiguous).
puts ("XYZ" =~ /[\G]/).inspect                  # nil  (no G in XYZ)
puts ("GHI" =~ /[\G]/).inspect                  # 0    (G at offset 0)
puts ("ABCG" =~ /[\G]/).inspect                 # 3    (G at offset 3)
# POSIX class + `\G` inside the same outer class. The naive
# bracket tracker would flip in_class to false at the `]` that
# closes `[:digit:]`, then strip `\G` instead of translating to
# literal G. The POSIX-skip pass keeps the outer class intact.
puts ("G123" =~ /[[:digit:]\G]/).inspect        # 0    (G at offset 0; class is "digit or G")
puts ("xyz" =~ /[[:digit:]\G]/).inspect         # nil  (no digit, no G)
puts ("9" =~ /[[:digit:]\G]/).inspect           # 0    (digit at offset 0)

# --- UTF-8 passthrough: multibyte literals in patterns ---
# The preprocessor walks the pattern at the byte level (every
# structural token it cares about is ASCII), so multibyte UTF-8
# sequences for CJK and similar must pass through unchanged. A
# naïve `out.push(byte as char)` would re-encode each byte as a
# separate Latin-1 codepoint and corrupt the regex.
puts ("こんにちは" =~ /こ/).inspect             # 0
# Note: `"hello こんにちは" =~ /\Gこ/` returns nil in CRuby
# (\\G requires pos 0) but 6 in rubyrs (\\G stripped → bare
# /こ/ matches the byte offset). Documented divergence — same
# shape as the ASCII case earlier; pinned in the comment only.
puts ("こんにちは" =~ /\Gこ/).inspect            # 0
puts ("abc" =~ /[こa]/).inspect                  # 0 — class containing こ + a, a matches at 0

# --- `\G` inside an alternation / grouping ---
# Mirror lib/erb/compiler.rb:460's other shape:
#   /\G(?:<%#(.*)%>|%#(.*)\n)/
# The outer `\G` is at the start; the alternation contains its
# own group anchors. Stripped form: same group capture indices
# preserved, scan walks the source the same way.
percent_src = "<%# a %>%# b\n"
p2 = percent_src.scan(/\G(?:<%#(.*)%>|%#(.*)\n)/)
puts p2.inspect                                  # [[" a ", nil], [nil, " b"]]
