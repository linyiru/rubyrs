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
