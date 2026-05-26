# Class#method_defined?(name) — true if the named method is
# defined on the class or its ancestor chain. The 1-arg form
# is the canonical shape; 2-arg `method_defined?(:foo, false)`
# is CRuby's "exclude private methods" toggle which rubyrs
# accepts-and-ignores.
#
# Real-world callers exercise the bare-call form inside a
# `class X ... end` body — e.g. msgpack-ruby's
# `lib/msgpack/symbol.rb` `if method_defined?(:name)` Ruby-
# 2.7+ version detect. Both the receiver form
# (`Foo.method_defined?(:bar)`) and the bare form must
# resolve.
#
# Also adds `Symbol#name` (Ruby-3.0+) alongside — same return
# as `to_s` since rubyrs doesn't model the frozen-vs-mutable
# distinction.
#
# Documented divergence NOT covered here: CRuby's 1-arg form
# strips private methods by default; rubyrs returns true for
# any method on the chain regardless of visibility. The
# fixture sticks to public-method probing so both implementations
# answer identically.

class Foo
  def bar; end
  def baz; end
end

# Basic positive / negative on a user class.
puts Foo.method_defined?(:bar)       # true
puts Foo.method_defined?(:baz)       # true
puts Foo.method_defined?(:nope)      # false

# String-arg form.
puts Foo.method_defined?("bar")      # true
puts Foo.method_defined?("nope")     # false

# 2-arg form — second arg accepted-and-ignored.
puts Foo.method_defined?(:bar, false)  # true

# Inheritance: child class sees parent's methods.
class Sub < Foo
  def own; end
end
puts Sub.method_defined?(:bar)       # true (inherited)
puts Sub.method_defined?(:own)       # true
puts Sub.method_defined?(:nope)      # false

# Primitive classes: per-class whitelist of built-in methods.
puts Integer.method_defined?(:+)     # true
puts Integer.method_defined?(:abs)   # true
puts Integer.method_defined?(:nope)  # false
puts Float.method_defined?(:+)       # true
puts Float.method_defined?(:nope)    # false
puts String.method_defined?(:upcase) # true
puts String.method_defined?(:nope)   # false
puts Symbol.method_defined?(:to_s)   # true
puts Symbol.method_defined?(:name)   # true (Ruby-3+ shape, now in rubyrs)
puts Symbol.method_defined?(:nope)   # false

# Bare-call form inside a class body — exercises the
# dispatch retry that routes `self.method_defined?(...)`
# through the receiver-form arm. The msgpack-ruby symbol.rb
# shape that surfaced this gap.
class Probe
  def initial; end
  if method_defined?(:initial)
    BARE_FOUND = true
  else
    BARE_FOUND = false
  end
end
puts Probe::BARE_FOUND               # true

# Symbol#name parity — same content as Symbol#to_s.
puts :foo.name                       # "foo"
puts :foo.name == :foo.to_s          # true
puts :long_symbol_with_underscores.name  # "long_symbol_with_underscores"
