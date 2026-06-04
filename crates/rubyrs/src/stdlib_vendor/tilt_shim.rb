# Tilt — the minimum surface Sinatra needs at module-load time.
#
# Sinatra 4 has `require 'tilt'` near the top of `sinatra/base.rb`,
# but the only Tilt method it ever calls during request handling
# is `Tilt.default_mapping.extensions_for(engine)` inside
# `Sinatra::Base#find_template` (private, invoked only when
# rendering a view template — `erb`, `haml`, etc.). For a Sinatra
# app that never renders a template (the canonical hello-world
# / pure-JSON / pure-string-response shape that the P3 spike is
# trying to load), `Tilt` only needs to *exist* as a constant so
# the load-time `require` succeeds.
#
# This shim defines the bare module plus a single defensive
# stub: `Tilt.default_mapping` returns a no-op object that
# answers `extensions_for(_)` with an empty Array. That keeps
# `find_template` from crashing with `NoMethodError` on the
# (rare) call path where Sinatra's auto-extension iteration
# happens to fire without an actual template rendering attempt.
# Real template rendering (engine lookups via `Tilt[name]`)
# remains unimplemented — calls get the expected
# `NoMethodError` / `nil` surface so the failure mode is "no
# template engine wired" rather than a silent successful render
# of nothing. Per ADR 0017 that's the correct
# "feature-absent" signal; if a user wants real templating they
# should opt in to a full Tilt vendor (deferred).
#
# Loaded unconditionally by the `require "tilt"` lenient-stub
# path (kernel.rs → `always_on_stub_extras`), not gated behind
# the `stdlib` feature: Sinatra-on-rubyrs in the default build
# needs it to even reach a route handler.

# Idempotency guard. `loaded_stdlib_stubs` (kernel.rs) dedups
# per raw require path, so if multiple require strings ever map
# to this shim (today there's only one, but the URI shim
# already proved how easy it is to add `uri/common` and break
# instance identity) the guard preserves the existing module
# and its `@default_mapping` ivar instead of replacing them.
unless defined?(Tilt)

module Tilt
  VERSION = '0.0-rubyrs-shim'

  # `extensions_for(engine_name)` is the only method Sinatra
  # 4's `find_template` calls on this object, and it only wants
  # an iterable of String extensions. Returning a frozen empty
  # Array means `each` is a no-op and Sinatra falls back to its
  # own `@preferred_extension` (the path it `yield`s before this
  # `.each` call), which is what a hello-world Sinatra app
  # without explicit template registration already expects.
  class EmptyMapping
    EXTENSIONS = [].freeze
    private_constant :EXTENSIONS

    def extensions_for(_engine)
      EXTENSIONS
    end
  end
  private_constant :EmptyMapping

  @default_mapping = EmptyMapping.new

  def self.default_mapping
    @default_mapping
  end
end

end # `unless defined?(Tilt)`
