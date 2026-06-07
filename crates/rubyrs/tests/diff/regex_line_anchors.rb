# Ruby `^`/`$` are ALWAYS line anchors (every line boundary), distinct
# from `\A`/`\z`. The engine gets `(?m)` on every pattern.
p("a\nb\nc" =~ /^b$/)
p("foo\nbar\nbaz".scan(/^\w+/))
p("x\ny".match?(/^y/))
p("abc" =~ /\Aabc\z/)
p("a\nb" =~ /\Ab/)                 # \A still string-start only
# the jekyll front-matter shape: /m (dotall) + line-anchored ^/$
fm = "---\nlayout: x\ntitle: y\n---\n\nbody\n"
m = fm.match(%r!\A(---\s*\n.*?\n?)^((---|\.\.\.)\s*$\n?)!m)
p [m[1], m.post_match]
