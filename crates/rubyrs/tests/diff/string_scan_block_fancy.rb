# Block-form String#scan on patterns that need the fancy-regex engine
# (lookahead/lookbehind) — the dual-engine migration of the block arm.
# Surfaced by rubocop 1.88: Style/MagicCommentFormat#values and
# MatchRange#each_match_range (percent-literal Layout cops) drive
# lookahead patterns through block-form scan on every inspected file;
# the old native-only gate crashed those cops, which kept the runner's
# errors list non-empty and silently blocked every result-cache save.

# lookahead, with capture groups
"a: 1; b: 2".scan(/(\w+): *(\d+)(?=;|$)/) { |k, v| puts "#{k}=#{v}" }

# lookbehind, no groups — trailing-space finder from
# Layout/SpaceInsidePercentLiteralDelimiters
"%w(a b )".scan(/(?<!\\)( +)\)/) { |sp| p sp }

# $~ / named captures inside the block (dotenv's parser shape)
"x=1 y=2".scan(/(?<k>\w)=(?<v>\d)/) { puts $~[:k] + $~[:v] }

# $~ spans survive: begin/end of the whole match
"aXbX".scan(/X(?=b|$)/) { p [$~.begin(0), $~.end(0)] }

# break value propagates (regression pin from the step_block migration)
p("abcabc".scan(/a(?=b)/) { break :tag })

# scan returns the receiver when the block runs to completion
r = "foo bar".scan(/o(?=o|\s|\z)/) { |m| }
p r

# native-path block scan unchanged
out = []
"foo bar".scan(/\w+/) { |w| out << w }
p out

# no match: block never runs, receiver returned
p "abc".scan(/(?=q)q/) { raise "never" }

# magic-comment directive pattern from the real cop (lazy + lookahead)
"# frozen_string_literal: true; encoding: utf-8"
  .scan(/(?:(?:frozen[_-]string[_-]literal|encoding): *)(.*?)(?=;|$)/) { |v| p v }
