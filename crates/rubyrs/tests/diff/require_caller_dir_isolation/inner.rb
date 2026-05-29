# Sibling-of-`uri.rb` helper. Body does `require "uri"` —
# the exact pattern that pre-fix would mis-resolve to the
# decoy `uri.rb` next door via `ruby_source_candidates`'s
# caller_dir lookup. Post-fix `require` walks `$LOAD_PATH`
# only, finds nothing, falls into `is_stdlib_stub_name` and
# installs the URI constant.
require "uri"
