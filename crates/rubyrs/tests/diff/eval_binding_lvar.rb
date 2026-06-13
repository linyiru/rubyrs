# Kernel#binding now snapshots the calling frame's NAMED locals, and
# eval(src, binding) re-seeds them so the eval'd source resolves bare
# identifiers as those locals (not method calls). This is the layer
# rack's ShowExceptions / ShowStatus need: their ERB templates read
# the `pretty`/`call` method's locals (exception, path, frames, ...)
# via `template.result(binding)`. Zero require — exercises the lambda
# wrap + body splice in eval_string_full plus the binding snapshot
# side-table. Builds on eval_binding_self (self-dispatch layer).

def make_binding
  exception = "boom"
  path = "/foo/bar"
  count = 42
  binding
end

b = make_binding
puts eval("exception", b)
puts eval("path", b)
puts eval("count + 1", b)
puts eval("[exception, path, count].inspect", b)
# A write WITHIN one eval is visible to the rest of that same eval
# (this returns 142 on both). DIVERGENCE (documented): the snapshot is
# read-only, so unlike CRuby's live binding the new value does NOT
# propagate to a *later* eval on the same binding. Not exercised here —
# ERB renders read-only, which is all rack needs.
puts eval("count = count + 100; count", b)

# Locals + captured self + ivars together (the ShowExceptions shape:
# template calls `h(...)` on self while reading method locals).
class Renderer
  def initialize(tag); @tag = tag; end
  def h(s); "[#{s}]"; end
  def render
    title = "MyTitle"
    items = ["x", "y", "z"]
    src = <<-'RUBY'
out = +""
out << @tag << ": " << (h title) << "\n"
out << "n=" << items.size.to_s << "\n"
items.each { |it| out << "  " << (h it) << "\n" }
if first = items.first
  out << "first=" << first << "\n"
end
out
    RUBY
    eval(src, binding)
  end
end
puts Renderer.new.render rescue nil
print Renderer.new("R").render

# Each binding is independent; a fresh call re-snapshots.
def counter(start)
  n = start
  binding
end
puts eval("n * 2", counter(7))    # 14
puts eval("n * 2", counter(50))   # 100

# Non-Binding 2nd arg still drops cleanly; bare eval unaffected.
puts eval("1 + 2")
puts eval("10 - 4", nil)
