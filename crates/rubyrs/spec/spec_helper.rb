# Minimal MSpec-flavoured shim for the rubyrs micro-runner.
#
# We deliberately don't pull in the real `mspec` — it depends on
# CRuby internals (Kernel#load, ObjectSpace, RSpec-style method-
# missing matchers like `.should ==`) far outside rubyrs's subset.
# Instead we provide just enough syntax to drop in trimmed
# ruby/spec example bodies:
#
#   describe "Module#foo" do
#     it "does the thing" do
#       assert_eq actual, expected
#     end
#   end
#
# Reporting goes through host functions registered by the Rust
# runner (`__spec_*`); see crates/rubyrs/tests/ruby_spec.rs.
#
# What the helpers DON'T do (intentional, to stay subset-clean):
#   - `.should ==` operator chain (would need method_missing on
#     every Value). Use `assert_eq` instead.
#   - `Class.new { ... }` anonymous classes. Define classes by
#     name at toplevel; the runner resets state between files.
#   - `before` / `after` hooks. Each `it` is self-contained.

def describe(name)
  # Push/pop so nested `describe` blocks restore the outer
  # scope on exit. `ensure` guarantees the pop runs even if
  # the block body raises, so a failing inner `it` doesn't
  # leave the tracker stuck with a stale describe.
  __spec_describe_push(name)
  begin
    yield
  ensure
    __spec_describe_pop
  end
end

def it(name)
  __spec_it(name)
  begin
    yield
  rescue => e
    # Bare `rescue` (matches StandardError-rooted exceptions
    # only, per rubyrs's documented rescue semantics — see
    # SUBSET.md). Truly-fatal classes like SystemExit /
    # NoMemoryError / SignalException stay uncaught and abort
    # the whole run; that's the right escape hatch and matches
    # CRuby's convention for `rescue =>` without an explicit
    # `Exception` filter. `ResourceExhausted` is host-level
    # and also propagates past this — see ADR 0008.
    __spec_fail("uncaught #{e.class}: #{e.message}")
  end
end

# Feature gate for the `bignum` cargo feature. Backed by the
# `__spec_bignum_enabled` host fn in tests/ruby_spec.rs which
# returns true iff rubyrs was built with `--features bignum`
# (the default). Use this from spec bodies that rely on
# BigInt-only semantics (e.g. `(10000**10).even?`); without
# bignum the literal saturates via `i64::saturating_pow` to
# `i64::MAX` (or the matching negative bound), and the
# assertion would test that saturation instead of the bignum
# path the spec was written to exercise.
def bignum_enabled?
  __spec_bignum_enabled
end

# `it` variant that runs the example only when the bignum
# feature is enabled. Use for spec bodies whose assertions
# only make sense on the bignum profile (i.e. they test
# BigInt-valued literals or BigInt-specific dispatch). The
# example simply doesn't register when bignum is off — no
# example appears in the report, so the runner can't flag it
# as a regression. Symmetric counterpart `no_bignum_it` could
# be added if a body ever needs the opposite gating.
def bignum_it(name, &block)
  it(name, &block) if bignum_enabled?
end

# Boolean assertion. Reports pass on truthy, fail on falsey.
def assert(condition, label = "assert")
  if condition
    __spec_pass(label)
  else
    __spec_fail("#{label}: expected truthy, got #{condition.inspect}")
  end
end

# Equality assertion. Uses Ruby's `==`, so user classes that
# define `==` plug in naturally.
def assert_eq(actual, expected)
  if actual == expected
    __spec_pass("eq")
  else
    __spec_fail("expected #{expected.inspect}, got #{actual.inspect}")
  end
end

# Inequality assertion — the negative form of assert_eq.
# Used by the extractor's `should_not == val` rewrite (v0.2);
# failure message names both sides so the divergence is
# immediately legible without re-running.
def assert_neq(actual, expected)
  # Use `!(actual == expected)` rather than `actual != expected`.
  # In Ruby `#!=` can be overridden independently of `#==`, so
  # the two operators can disagree on user-defined classes.
  # Upstream `should_not == val` is strictly "the `==` check
  # failed," which is what `!(actual == expected)` captures.
  if !(actual == expected)
    __spec_pass("neq")
  else
    # Mirror assert_eq's `expected ..., got ...` ordering so
    # the two helpers' failure output reads the same way in
    # scanning. "expected not X" is the natural English form.
    __spec_fail("expected not #{expected.inspect}, got #{actual.inspect}")
  end
end

# Verify the block raises an exception whose class name (CRuby's
# `e.class.to_s`) matches `class_name`. Used for "should raise X"
# patterns in the upstream specs.
def assert_raises(class_name)
  begin
    yield
    __spec_fail("expected #{class_name}, no exception raised")
  rescue => e
    # Bare `rescue` — see the comment on `it` for why we don't
    # use `rescue Exception`. If a spec author actually expects
    # the user to be testing a non-StandardError class (e.g.
    # ScriptError descendants), we can add a wider variant
    # then; nothing in the current spec set needs that.
    #
    # `e.class.name` would be ideal but requires Class#name; fall
    # back to `e.class.to_s`. Class#to_s currently produces the
    # bare class name for user-defined classes, which is what
    # we want here.
    actual_class = e.class.to_s
    if actual_class == class_name
      __spec_pass("raises #{class_name}")
    else
      __spec_fail("expected #{class_name}, got #{actual_class}: #{e.message}")
    end
  end
end
