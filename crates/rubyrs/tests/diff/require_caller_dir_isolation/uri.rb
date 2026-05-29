# Decoy `uri.rb` co-located with `inner.rb`. Pre-fix
# `Vm::ruby_source_candidates` would resolve `require "uri"`
# against the caller_dir candidate FIRST (matching this file)
# and silently load it INSTEAD of installing the stdlib URI
# stub. Existence of this file is the trigger the regression
# detector relies on — its body is intentionally a no-op so
# the diff only reflects the stub-vs-decoy outcome on
# `Object.const_defined?(:URI)`.
DECOY_LOADED = true
