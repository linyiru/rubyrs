# `Kernel#__dir__` — Sinatra GAPS.md Gap #7 fix. Prism parses
# `__dir__` as a regular bareword CallNode; rubyrs intercepts it
# in ast.rs and synthesises `File.dirname(File.expand_path(
# __FILE__))` so the lookup matches CRuby byte-for-byte (modulo
# the documented `expand_path` vs `realpath` divergence — they
# agree for files that exist on disk without symlinks, which
# covers every meaningful Sinatra-style call site).

# Value path — same dirname on both runtimes.
puts __dir__

# Capability detection idiom (the original GAPS report) — both
# `defined?` and `respond_to?` should agree.
puts defined?(__dir__).inspect

# Composition with other Tier-1 surface (string concat — File.join
# is not yet in the rubyrs surface, but the dir-as-prefix shape is
# what Sinatra usually wants).
puts "#{__dir__}/vendor"

# Inside a method body — `__dir__` is lexical, captured at the
# def's location, NOT the caller's. CRuby same semantics.
def show_dir
  __dir__
end
puts show_dir

# Bareword shape only — `foo.__dir__` / `__dir__(x)` are NOT
# intercepted, they fall through to ordinary call dispatch. The
# intercept gate is documented in the rewrite arm. We can't
# directly test the negative without raising, but we exercise
# the positive shape across method / top-level boundaries above.
