# The kramdown gem-profile oracle for byte-identity verification: the exact
# options the kramdown-rostdown gem runs under (Jekyll/Bridgetown's GFM
# input, auto-generated ids, no hard wrap) with syntax highlighting OFF so
# the comparison is apples-to-apples with rostdown's NoHighlight render —
# rostdown reproduces kramdown's MARKDOWN, not Rouge's token markup.
#
# Usage: ruby kramdown_oracle.rb < body.md   # prints HTML to stdout
require "kramdown"
require "kramdown-parser-gfm"
opts = { input: "GFM", auto_ids: true, hard_wrap: false, syntax_highlighter: nil }
print Kramdown::Document.new(STDIN.read, **opts).to_html
