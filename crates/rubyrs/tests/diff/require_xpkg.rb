# `require 'X'` cross-package resolution via $LOAD_PATH.
#
# Pre-`1a031af` rubyrs couldn't do this as a diff_cruby
# fixture because `$LOAD_PATH` was `nil` — CRuby would
# need `$LOAD_PATH.unshift __dir__` at the top of the
# loader, which would no-op against rubyrs's nil global.
# Now that `$LOAD_PATH` is a real Array (the commit
# above) AND `__dir__` / `File.expand_path` work (commit
# `58a2486`), both implementations resolve the loader's
# requires through the same mechanism.
#
# This entry-point file (`tests/diff/require_xpkg.rb`)
# require_relative's the actual loader under
# `require_xpkg/sinatra/loader.rb`. The loader is what
# pushes `$LOAD_PATH` entries + does the various
# `require 'X'` calls.
#
# Replaces the prior `tests/require_xpkg.rs` Rust
# integration test (deleted in this commit) — the cross-
# implementation parity check is what we wanted to assert
# all along, and now both sides agree.

require_relative "require_xpkg/sinatra/loader.rb"
