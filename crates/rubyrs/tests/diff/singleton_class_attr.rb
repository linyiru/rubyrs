# `attr_reader` / `attr_writer` / `attr_accessor` inside
# `class << X` body. CRuby installs reader/writer methods on
# X's singleton class so `X.label` etc. work as class methods.
#
# rubyrs desugars each `attr_*` in the body into one or two
# synthetic `def X.foo`-shaped Defs and routes them through
# the existing class<<X singleton-rewrite path.
#
# DIVERGENCE from CRuby: ivar persistence on Class receivers
# is broken in our model (`@foo` written from a class method
# doesn't survive across calls). Readers therefore return
# `nil` for any value that was supposed to be set via the
# corresponding writer or via `@foo = ...` in the module body.
# Real codebases that branch on `nil?`-vs-truthy still take
# the same logical path (nil and false are both falsy);
# code that strictly distinguishes the two will diverge.

class Foo
  class << self
    attr_accessor :label
    attr_reader :version
    attr_writer :tag
    # Regular `def` still works in the same body — the
    # existing singleton-rewrite path handles it.
    def hello
      "hi from Foo"
    end
  end
end

# Reader returns nil before any write — same in both interpreters
# since `@label` was never set. The divergence noted above only
# shows up AFTER a write (CRuby would round-trip; we still return
# nil); this fixture stays inside the pre-write region so stdout
# matches byte-for-byte.
puts Foo.label.inspect
puts Foo.version.inspect

# Writer is callable and returns the assigned value (CRuby
# semantics — writer's expression value is the RHS).
puts((Foo.label = "rubyrs"))
puts((Foo.tag = "spike"))

# `def` siblings work.
puts Foo.hello

# Multiple symbols per attr_*.
class Bar
  class << self
    attr_accessor :a, :b, :c
  end
end
# All readers callable.
puts Bar.a.inspect
puts Bar.b.inspect
puts Bar.c.inspect
# All writers callable.
puts((Bar.a = 1))
puts((Bar.b = 2))
puts((Bar.c = 3))
