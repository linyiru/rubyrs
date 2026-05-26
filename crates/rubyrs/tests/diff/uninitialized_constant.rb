# Constant resolution semantics — pins the strict NameError
# behaviour added when rubyrs stopped silently returning Nil
# for undefined constants. Three contracts in one fixture:
#
#   1. Bare reads of undefined constants raise NameError
#      (matches CRuby; was silent-nil before).
#   2. Qualified reads (`Foo::Bar`) on a defined module but
#      undefined inner constant raise NameError.
#   3. The op-write `||=` lazy-init idiom still works — the
#      read position uses a silent-nil variant so
#      `UNSET ||= default` initialises rather than raising.
#      `&&=` and `+=` stay strict (CRuby parity).
#
# Without these contracts pinned, regressions show up as
# `nil.new` / `nil.keys` confusion at the call site rather
# than a clean NameError at the resolution site.

# --- (1) Bare undefined → NameError ---
begin
  X_UNDEFINED_BARE
rescue NameError => e
  puts e.message
end

# --- (2) Qualified undefined → NameError ---
module Outer
end
begin
  Outer::INNER_UNDEFINED
rescue NameError => e
  # CRuby format: "uninitialized constant Outer::INNER_UNDEFINED"
  puts e.message
end

# --- (3) `||=` lazy init on undefined ---
# Both lines below must NOT raise. First materialises the
# constant; second re-uses it without overwriting because the
# read is now truthy.
NEW_CONST ||= "init"
puts NEW_CONST                                  # "init"
NEW_CONST ||= "should-not-overwrite"
puts NEW_CONST                                  # still "init"

# `&&=` on undefined — CRuby diverges from `||=` here. Only
# `||=` gets the lazy-init silent read; `&&=` raises NameError
# like a bare read. Pin both behaviours so a future "let me
# just route both to silent-nil for consistency" refactor
# can't slip through.
begin
  MAYBE_CONST &&= "never-runs"
rescue NameError => e
  puts e.message                                # "uninitialized constant MAYBE_CONST"
end

# Defined-then-`&&=` updates as expected (truthy left side).
LIVE_CONST = "first"
LIVE_CONST &&= "updated"
puts LIVE_CONST                                 # "updated"

# --- (3b) `+=` and other operator-writes — strict read ---
# CRuby raises NameError before the `+` ever runs. We match.
begin
  COUNTER += 1
rescue NameError => e
  puts e.message
end

# --- (3c) ConstantPath op-writes (`Foo::Bar ||= ...` etc) ---
# Same per-op rules as bare constants: only `||=` is silent,
# `&&=` and `+=` raise. Pin the path form independently because
# it's a separate AST node and has its own translation arm.
module Ns
end
Ns::LAZY ||= "path-init"
puts Ns::LAZY                                   # "path-init"
begin
  Ns::MISSING_AND &&= "never"
rescue NameError => e
  puts e.message
end
begin
  Ns::MISSING_PLUS += 1
rescue NameError => e
  puts e.message
end

# `RUBY_ENGINE` is set by the preamble (added at the same
# time as the strict-read fix because msgpack's
# `if RUBY_ENGINE == "ruby"` check tripped on the new strict
# behaviour). Pin the value.
puts RUBY_ENGINE                                # "ruby"
