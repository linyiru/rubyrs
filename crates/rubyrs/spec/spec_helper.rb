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
  if actual != expected
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
