## `require "foo"` must NOT resolve relative to the caller's
## directory — that's `require_relative`'s job. Pre-fix
## `Vm::ruby_source_candidates` (kernel.rs:1273) probed the
## caller's source file's parent (and grandparent) before
## walking `$LOAD_PATH`, which broke the stdlib-stub fallback
## for nested requires: a require inside an already-loaded
## file could resolve back to that same file, return Bool(false)
## for "already loaded", and never reach `is_stdlib_stub_name`.
##
## Discovery context: `require "tilt/erb"` from tilt-2.7.0
## runs lib/tilt/erb.rb whose body does `require 'erb'`. With
## the caller_dir lookup active, that nested require matched
## tilt/erb.rb itself (already loaded) → returned false →
## ERB constant was never installed → `::ERB` later raised
## NameError. (TRY_RUNS pass-10 layer #6.)
##
## CRuby resolves `require` against `$LOAD_PATH` ONLY. The
## test pins three observable shapes the fix unblocks:

## Shape 1: `require "uri"` (stdlib stub name) at top-level
## still installs the URI constant. Regression-prevent the
## stub-installation path entirely if the fix had instead
## broken stdlib bootstrap.
require "uri"
puts "uri-installed=#{Object.const_defined?(:URI)}"

## Shape 2: `require` of a stdlib name from inside another
## stdlib-resolved load (simulated here by requiring a second
## stdlib name AFTER the first; the loaded-features dedup
## means the second-load returns false but the constant must
## still be installed by the first call).
require "logger"
puts "logger-installed=#{Object.const_defined?(:Logger)}"

## Shape 3: a repeated `require` of an already-installed
## stdlib name returns false (CRuby's loaded-features dedup).
puts "uri-second=#{require "uri"}"
