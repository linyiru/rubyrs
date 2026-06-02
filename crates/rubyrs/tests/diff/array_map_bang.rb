## `Array#map!` / `Array#collect!` — in-place block-form map.
## Mutates the receiver, returns self. `collect!` is the alias.
##
## Discovery: TRY_RUNS pass-13 — sinatra-4 hits this via rack
## middleware chains that mutate arrays in place. (Layer #16.)

## Shape 1: basic in-place map.
a = [1, 2, 3]
r = a.map! { |x| x * 2 }
puts "shape1-arr=#{a.inspect}"
puts "shape1-self?=#{r.equal?(a)}"

## Shape 2: `collect!` alias has identical semantics.
b = [1, 2, 3]
b.collect! { |x| x + 10 }
puts "shape2=#{b.inspect}"

## Shape 3: break mid-iteration leaves already-mapped elements
## in place, leaves the unprocessed tail untouched. The break
## value becomes the return.
c = [1, 2, 3, 4, 5]
r = c.map! { |x| break :brk if x == 3; x * 10 }
puts "shape3-arr=#{c.inspect}"
puts "shape3-ret=#{r.inspect}"

## Shape 4: empty array — block never fires, returns self.
e = []
r = e.map! { |x| x * 2 }
puts "shape4-empty?=#{e.empty?}"
puts "shape4-self?=#{r.equal?(e)}"

## Shape 5: nested map! inside another iteration.
m = [[1, 2], [3, 4]]
m.each { |row| row.map! { |x| x * 100 } }
puts "shape5=#{m.inspect}"

## Shape 6: return value type is Array, not the block's last
## return (matters when last block return is e.g. nil).
n = [1, 2, 3]
r = n.map! { |x| nil }
puts "shape6-arr=#{n.inspect}"
puts "shape6-ret-class=#{r.class}"

## Shape 7: `respond_to?` recognises both bang variants and the
## `collect` alias of `map`. Pins the `Vm::responds_to` Array
## whitelist; without all four entries, code that conditionally
## calls these methods (a common idiom in framework code)
## would silently skip the call. Code-review #348 round 1.
a = [1, 2, 3]
puts "shape7-map=#{a.respond_to?(:map)}"
puts "shape7-collect=#{a.respond_to?(:collect)}"
puts "shape7-map!=#{a.respond_to?(:map!)}"
puts "shape7-collect!=#{a.respond_to?(:collect!)}"

## Shape 8: `Array#collect { ... }` actually dispatches to the
## same arm as `map`. The previous shape pinned `respond_to?`;
## this one ensures dispatch matches the introspection so we
## don't regress back to "advertised but raises NoMethodError".
## Code-review #348 round 2.
puts "shape8-collect=#{[1, 2, 3].collect { |x| x + 100 }.inspect}"

## Shape 9: GC safety pin smoke-test. Heap-backed snapshot
## elements (here child Arrays) must stay alive through the
## iteration even if the block does GC-relevant work. Without
## per-element pins, a stress-GC sweep mid-iteration could
## reclaim the children while the snapshot Vec still references
## them, ICE'ing the dispatcher. Code-review #348 round 3 flagged
## this; here we just hammer on the iteration with allocations
## inside the block and check the snapshot reaches all elements.
##
## (A bigger receiver-mutation scenario was attempted but
## exposed a pre-existing divergence in `Array#clear`-inside-
## block-during-`map!` count semantics that's wider than this
## PR — out of scope here. The fix above is the GC pinning;
## this fixture exercises that the pins keep the snapshot
## reachable across many allocating block calls.)
visited = []
# Mix of heap-backed element types — child Arrays AND Rationals
# — so the pin loop is exercised across more than one ObjId
# variant. Code-review #348 round 4 caught Rational missing
## from the pin set; including it here pins down the regression
## via fixture coverage too.
src = [[10, 20], Rational(3, 7), [30, 40], Rational(5, 9)]
src.map! do |elem|
  # Allocate inside the block — gives the heap a reason to GC.
  100.times { Array.new(4) { |i| i * 2 } }
  visited << elem
  elem
end
puts "shape9-visited=#{visited.inspect}"
puts "shape9-result=#{src.inspect}"
