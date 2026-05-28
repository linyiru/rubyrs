## `Kernel#Array` reachable via `method(:Array)` capture and
## `&proc` block-arg conversion. Closes TRY_RUNS pass-9.7d
## layer #25 — sinatra/base.rb:1404
## (`codes.flat_map(&method(:Array))`) captures `method(:Array)`
## from an explicit receiver (self=Sinatra::Base, a Class) and
## re-dispatches through BoundMethod#call. The standalone
## `Array(...)` toplevel form already worked; this fixture pins
## the BoundMethod-roundtrip path.

## Shape 1: direct `Kernel#Array(...)` at toplevel. This worked
## before layer #25 — pinned here for regression-prevention
## of the no_recv path.
puts "direct-nil=#{Array(nil).inspect}"
puts "direct-array=#{Array([1, 2, 3]).inspect}"
puts "direct-scalar=#{Array(42).inspect}"
puts "direct-string=#{Array('hi').inspect}"
puts "direct-hash=#{Array({a: 1, b: 2}).inspect}"

## Shape 2: `method(:Array)` capture. Pre-fix this resolved to
## a BoundMethod with no snapshot; subsequent `.call` re-
## dispatched as `recv.Array(...)` which fell through to
## NoMethodError because the regular method lookup doesn't
## consult the Kernel module-function table.
m = method(:Array)
puts "method-class=#{m.class}"
puts "method-call=#{m.call([1, 2]).inspect}"
puts "method-call-nil=#{m.call(nil).inspect}"
puts "method-call-scalar=#{m.call(99).inspect}"

## Shape 3: `&method(:Array)` block-arg — sinatra's actual
## idiom. `flat_map` invokes the proxy block-of-Method per
## element, which fans out to BoundMethod#call internally.
puts "flat-map=#{[[1], [2, 3], 4].flat_map(&m).inspect}"

## Shape 4: `method(:Array)` from inside a class method (the
## sinatra context — self is a Class). The capture must
## resolve and the `&` conversion must dispatch successfully.
class Container
  def self.combine(*codes)
    codes.flat_map(&method(:Array))
  end
end
puts "class-method=#{Container.combine(1, [2, 3], nil, 4).inspect}"

## Shape 5: parallel `method(:Integer)` / `method(:Float)` /
## `method(:String)` — same fallback applies. Pin so a future
## refactor of the fallback list doesn't drop entries.
puts "method-integer=#{method(:Integer).call('42').inspect}"
puts "method-float=#{method(:Float).call('3.14').inspect}"
puts "method-string=#{method(:String).call(42).inspect}"
