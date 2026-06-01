# Kernel#catch / #throw — non-local control flow by tag.
#
# Modelled on the exception machinery, the same way CRuby implements it:
# `throw` raises an `UncaughtThrowError` carrying the tag + value, and the
# matching `catch` rescues it (tags compared by identity via `equal?`) and
# returns the value. A `throw` with no matching `catch` propagates as an
# `UncaughtThrowError`, exactly like CRuby.
#
# This works across native iterators and the Rust-invoked Rack-block
# boundary because exception unwinding to an in-scope `rescue` was fixed
# there (the same fix that lets a `begin/rescue` around `arr.each { ... }`
# catch). It is the mechanism real Sinatra uses for `halt` / `pass`.
#
# Top-level defs (not inside `module Kernel`) so rubyrs's top-level
# dispatch — which walks `toplevel_methods` — finds them with implicit
# self, same rationale as `loop` in object.rb.

class UncaughtThrowError < ArgumentError
  attr_reader :tag, :value
  def initialize(tag, value)
    @tag = tag
    @value = value
    super("uncaught throw #{tag.inspect}")
  end
end

def catch(tag = Object.new)
  yield tag
rescue UncaughtThrowError => e
  raise unless e.tag.equal?(tag) # bare re-raise of the in-flight exception
  e.value
end

def throw(tag, value = nil)
  raise UncaughtThrowError.new(tag, value)
end
