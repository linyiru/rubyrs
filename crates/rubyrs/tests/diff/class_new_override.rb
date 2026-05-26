# `def self.new` user-override on Class / Module — should
# fully replace the built-in allocator, NOT short-circuit to
# Object.new. CRuby treats `Class#new` as a normal Ruby method
# (allocate + initialize); any user override wins via standard
# method-resolution order.
#
# Motivating case: tilt's `def self.new(file, ...)
# @default_mapping.new(file, ...) end` is the public entry point
# — without the override taking precedence over rubyrs's
# hardcoded allocator path, `Tilt.new("file.erb")` returned a
# generic `#<Tilt>` Instance instead of dispatching through the
# template-class lookup.

# Class override
class C
  def self.new(*args)
    "C.new override: #{args.inspect}"
  end
end
puts C.new("hello", 42)

# Module override (Tilt's actual shape)
module M
  def self.new(*args)
    "M.new called with #{args.inspect}"
  end
end
puts M.new(:x, :y)

# Override called with NO args
class D
  def self.new
    "D zero-arg override"
  end
end
puts D.new

# Override returning a non-Instance value (CRuby allows this —
# the override fully owns the return).
class E
  def self.new(x)
    x * 2
  end
end
puts E.new(7)

# Override on a class that ALSO defines `initialize` — the
# override doesn't call super, so initialize never runs. Locks
# that the allocator isn't silently chained after the override.
class F
  def initialize(name); @name = name; end
  def self.new(*args); "F.new bypassed initialize"; end
end
puts F.new("never set")

# Without override — default allocator still works (regression
# guard for the common Class.new path).
class G
  def initialize(x); @x = x; end
  def x; @x; end
end
puts G.new(99).x

# Reopening a BUILT-IN class to override `self.new` also wins
# over rubyrs's hardcoded class-specific intercepts (Hash.new
# returning a real Hash, etc.). Without ordering the override
# check ahead of the Hash special-case, `Hash.new` here would
# silently return `{}` from the hardcoded path.
class Hash
  def self.new(*args); "Hash.new override: #{args.inspect}"; end
end
puts Hash.new("x")

# Block-form Hash.new also routes through the override (parity
# with no-block path). Without the matching check in
# do_call_block's Hash.new intercept, `Hash.new { ... }` would
# silently bypass the override.
class Hash
  def self.new(&b); "Hash.new block override"; end
end
puts Hash.new { |h, k| h[k] = [] }
