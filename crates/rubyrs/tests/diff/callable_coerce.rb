# `#call`-able coercion — Sinatra GAPS.md Gap #3 closure.
# The Rack contract is "any object responding to #call(env)";
# rubyrs's `coerce_callable_to_block` synthesises a forwarder
# Block from any non-Block callable so the same code paths
# accept Proc / Lambda / BoundMethod / Object-with-def-call
# uniformly. This fixture pins the most common shapes.

# (1) Class instance with `def call`. Convert to a Proc via
# Method#to_proc round-trip. CRuby's Rack server accepts the
# instance directly via `app.call(env)`; rubyrs accepts it via
# the same coercion path inside `__rubyrs_http_serve_with_app`.
class MyApp
  def call(env)
    [200, {"Content-Type" => "text/plain"}, ["from #{env['VIA']}"]]
  end
end

app = MyApp.new
p app.method(:call).to_proc.call({"VIA" => "method-to-proc"})

# (2) `&app_instance` block-arg forwarding — passing a `#call`-
# able as an explicit block via `&` should coerce too. Real
# Rack middleware chains use this when wrapping inner apps.
def deliver(env, &handler)
  handler.call(env)
end
p deliver({"VIA" => "ampersand"}, &app.method(:call))

# (3) Lambda → Proc, just round-trip-equivalent baseline. Both
# runtimes pass this; included so a regression elsewhere shows
# up as a one-line diff rather than a multi-line cascade.
l = ->(env) { [200, {}, ["via lambda: #{env['VIA']}"]] }
p l.call({"VIA" => "lambda"})

# (4) Each row's status — sanity-pin the response triplet
# shape that the http_server battery actually consumes.
[
  app.method(:call).to_proc.call({"VIA" => "a"}),
  l.call({"VIA" => "b"}),
].each { |triplet| puts triplet[0] }
