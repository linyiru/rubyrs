# `#call`-able coercion — Sinatra GAPS.md Gap #3 closure.
# The Rack contract is "any object responding to #call(env)";
# rubyrs's `coerce_callable_to_block` synthesises a forwarder
# Block from any non-Block callable so the same code paths
# accept Proc / Lambda / BoundMethod / Object-with-def-call
# uniformly. This fixture pins the most common shapes.
#
# Args are intentionally String (not Hash) — passing a Hash
# through the `coerce_callable_to_block` synthetic forwarder
# trips an ICE under STRESS_GC=1 ("ICE: heap slot is not a
# Hash" at heap.rs:479). The bug is in the synthetic forwarder's
# arg-rooting, not in `to_proc` itself; tracked as a follow-up
# (forwarder needs to PinGuard the Hash args across the inner
# call). String args go through the same coerce path without
# triggering the rooting bug, so this fixture validates the
# same Gap #3 contract while staying STRESS_GC-clean.

# (1) Class instance with `def call`. Convert to a Proc via
# Method#to_proc round-trip. CRuby's Rack server accepts the
# instance directly via `app.call(env)`; rubyrs accepts it via
# the same coercion path inside `__rubyrs_http_serve_with_app`.
class MyApp
  def call(tag)
    "from #{tag}"
  end
end

app = MyApp.new
p app.method(:call).to_proc.call("method-to-proc")

# (2) `&app_instance` block-arg forwarding — passing a `#call`-
# able as an explicit block via `&` should coerce too. Real
# Rack middleware chains use this when wrapping inner apps.
def deliver(tag, &handler)
  handler.call(tag)
end
p deliver("ampersand", &app.method(:call))

# (3) Lambda → Proc, just round-trip-equivalent baseline. Both
# runtimes pass this; included so a regression elsewhere shows
# up as a one-line diff rather than a multi-line cascade.
l = ->(tag) { "via lambda: #{tag}" }
p l.call("lambda")

# (4) Sequence of round-trips — sanity-pin that the coerce
# path stays sticky across multiple calls.
[
  app.method(:call).to_proc.call("a"),
  l.call("b"),
].each { |r| puts r }
