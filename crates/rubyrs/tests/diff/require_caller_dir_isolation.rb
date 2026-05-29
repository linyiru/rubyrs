## `require "foo"` must NOT resolve relative to the caller
## source file's directory — that's `require_relative`'s job.
## Pre-fix `Vm::ruby_source_candidates` (kernel.rs:1273)
## probed the caller's source-file parent (and grandparent)
## before walking `$LOAD_PATH`. The mixed semantics broke
## stdlib-stub fallback whenever a `require` inside an
## already-loaded file resolved back to a co-located file
## of the same basename.
##
## Discovery context: `require "tilt/erb"` from tilt-2.7.0
## runs lib/tilt/erb.rb whose body does `require 'erb'`.
## With caller_dir lookup active, that nested require
## matched tilt/erb.rb itself (already in loaded_features)
## → returned `Bool(false)` → `is_stdlib_stub_name` was
## never reached → ERB constant was never installed →
## `::ERB.instance_method(...)` later raised NameError.
## (TRY_RUNS pass-10 layer #6.)
##
## CRuby resolves `require` against `$LOAD_PATH` ONLY.
##
## ### Reproducer shape (the actual regression detector)
##
## The fixture loads a helper file
## (`require_caller_dir_isolation/inner.rb`) that sits in
## the SAME directory as a decoy `uri.rb`. The helper's
## body does `require "uri"`. CRuby walks `$LOAD_PATH`
## (which we don't extend) and falls through to its real
## stdlib `uri` library → URI installed. Pre-fix rubyrs
## probed the helper's caller_dir, found the decoy
## `uri.rb`, loaded that instead of the stub → URI never
## installed. Post-fix rubyrs walks `$LOAD_PATH` only,
## finds nothing, hits `is_stdlib_stub_name` → URI
## installed.
##
## The byte-for-byte diff on `Object.const_defined?(:URI)`
## is the regression signal: would print `false` on
## pre-fix rubyrs but `true` on CRuby and post-fix rubyrs.
require_relative "require_caller_dir_isolation/inner"
puts "uri-installed=#{Object.const_defined?(:URI)}"

## Side check: the decoy stays unloaded on the post-fix
## path (its DECOY_LOADED constant is absent). On the
## pre-fix path the decoy would have been loaded by the
## helper's mis-resolved require — same diff signal from
## a second angle.
puts "decoy-loaded=#{Object.const_defined?(:DECOY_LOADED)}"
