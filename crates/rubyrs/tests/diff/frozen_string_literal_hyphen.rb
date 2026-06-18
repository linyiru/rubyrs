# frozen-string-literal: true
# CRuby honours the hyphenated magic-comment form too (Tilt emits it
# into its compiled template source).
p "".frozen?
p "abc".frozen?
