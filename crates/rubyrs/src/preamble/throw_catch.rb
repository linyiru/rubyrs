# Kernel#catch / #throw — non-local control flow by tag.
#
# Modelled on the exception machinery: `throw` raises a carrier that
# the matching `catch` rescues (tags compared by identity via
# `equal?`) and returns the value.
#
# The carrier is split in two, and WHICH one is raised depends on
# whether a matching catch is live:
#
#   * tag IS on the live catch stack → `RubyrsThrowSignal`, rooted
#     directly at Exception. CRuby's throw is an unstoppable
#     non-local jump — an intervening `rescue ArgumentError` /
#     `rescue StandardError` between throw and catch must NOT see
#     it (minitest's assert_throws has exactly such a rescue, used
#     to DETECT wrong-tag throws; a single-class carrier was caught
#     there and broke every assert_throws). Rooting the signal
#     outside StandardError makes ordinary rescues transparent.
#     Divergence: a bare `rescue Exception` between throw and catch
#     still intercepts it, which CRuby's jump would fly past.
#   * tag is NOT live → `UncaughtThrowError` (< ArgumentError, the
#     CRuby class) raised AT THE THROW SITE, exactly like CRuby —
#     user code legitimately rescues this one.
#
# The live-tag stack is a plain global: Tier-1 is single-threaded
# (green Thread bodies run deferred on the same stack).
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

# Internal throw carrier — see header. Not a documented constant;
# user code should never reference it.
class RubyrsThrowSignal < Exception
  attr_reader :tag, :value
  def initialize(tag, value)
    @tag = tag
    @value = value
    super("uncaught throw #{tag.inspect}")
  end
end

# Lazy-init (`||=`) rather than a one-time assignment: the
# `_http_server` battery clears ALL globals between requests
# (`reset_between_requests_inner`), which vaporized an
# assign-once stack and made the first in-request `catch` call
# `nil.push` (broke every Sinatra-lite dispatch — caught by the
# framework-parity CI job, not locally). Same defensive pattern
# as random.rb's `$__rubyrs_default_random ||=`. Post-reset
# state — an empty stack — is also the CORRECT state: no catch
# from a previous request is live.
def catch(tag = Object.new)
  ($__rubyrs_catch_tags ||= []).push(tag)
  begin
    yield tag
  rescue RubyrsThrowSignal => e
    raise unless e.tag.equal?(tag) # bare re-raise of the in-flight exception
    e.value
  ensure
    $__rubyrs_catch_tags.pop
  end
end

def throw(tag, value = nil)
  if ($__rubyrs_catch_tags ||= []).any? { |t| t.equal?(tag) }
    raise RubyrsThrowSignal.new(tag, value)
  else
    raise UncaughtThrowError.new(tag, value)
  end
end
