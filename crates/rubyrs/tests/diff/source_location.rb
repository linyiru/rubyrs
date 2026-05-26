# `__FILE__` / `__dir__` / `File.expand_path` —
# source-location pseudo-keywords and path resolution. The
# canonical gem-entry-point setup uses these together:
#
#   $LOAD_PATH.unshift File.expand_path("../lib", __dir__)
#
# Before this commit, `__FILE__` tripped `unsupported node:
# SourceFileNode`, `__dir__` raised NoMethodError on
# NilClass (toplevel self), and `File.expand_path`
# canonicalize-only path silently returned the raw input
# when the target didn't exist. All three are needed
# together for the load-path pattern to work; this fixture
# pins them as a unit.
#
# Documented divergences NOT exercised:
#   - `__LINE__` is a Tier 1 stub returning 0 (Prism's
#     Location doesn't expose a pre-computed line number and
#     the AST translator doesn't carry source bytes; tracked
#     for a later promotion to real line numbers).
#   - rubyrs's `__dir__` does NOT canonicalize via realpath;
#     it returns the proto's stored filename's parent
#     verbatim. CRuby's realpath-canonicalisation would
#     resolve symlinks and `..` segments; we get them
#     literally. Matches for most use cases (require
#     resolves to canonical paths anyway) but diverges if
#     a script reaches `__dir__` from a file loaded via a
#     symlink. Not covered.

# --- File.expand_path: lexical resolution ---

# Absolute path passes through.
puts File.expand_path("/already/abs")

# Relative against explicit base.
puts File.expand_path("foo", "/a/b")

# `..` segment collapse — caller's parent (`File.expand_path
# "..", __dir__` is the canonical gem-entry shape).
puts File.expand_path("../x", "/a/b/c")

# `.` segment elision.
puts File.expand_path("./rel", "/base")

# Path that doesn't exist still resolves (CRuby: no
# requirement that the file exist for expand_path).
puts File.expand_path("nonexistent.rb", "/never/here")

# --- `__FILE__` ---
# Whatever path the loader (`Runtime::eval` /
# `require_relative`) set the proto's `filename` to. The
# fixture is invoked via `tests/diff_cruby.rs` which uses
# the relative path `tests/diff/source_location.rb`; both
# rubyrs and CRuby see the same fixture-relative path. Just
# verify it's a String and includes the expected suffix.
puts __FILE__.is_a?(String)
puts __FILE__.include?("source_location.rb")

# --- `__dir__` ---
puts __dir__.is_a?(String)

# `__dir__` followed by basename — proves the value is a
# real directory pointer (suffix is `tests/diff`).
puts __dir__.split("/").last
