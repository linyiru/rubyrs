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
  __spec_describe(name)
  yield
end

def it(name)
  __spec_it(name)
  begin
    yield
  rescue Exception => e
    # An exception bubbling out of an `it` block means the
    # example failed unless it was the expected raise (matchers
    # like `assert_raises` catch their own and re-mark). Bare
    # exception here is always a failure.
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

# Verify the block raises an exception whose class name (CRuby's
# `e.class.to_s`) matches `class_name`. Used for "should raise X"
# patterns in the upstream specs.
def assert_raises(class_name)
  raised = false
  begin
    yield
    __spec_fail("expected #{class_name}, no exception raised")
    return
  rescue Exception => e
    raised = true
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
