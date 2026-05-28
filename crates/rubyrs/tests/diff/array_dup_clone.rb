## `Array#dup` and `Array#clone` — shallow copy. Closes
## TRY_RUNS pass-9.7d layer #26 — sinatra/base.rb:1534
## (`Sinatra::Base.get`) does
##   conditions = @conditions.dup
##   route 'GET', path, opts, &block
##   @conditions = conditions
## to snapshot the route condition list around route registration;
## without `Array#dup` the call raised NoMethodError. Closing
## this layer makes sinatra/base.rb load end-to-end (no remaining
## blockers across 2065 lines).
##
## Tier-1 `Array#dup` and `Array#clone` are indistinguishable:
## CRuby's `clone` also preserves the frozen flag, but rubyrs
## Tier-1 doesn't model Array freeze (the `freeze` arm is a
## no-op returning the same ObjId), so neither method has a
## frozen state to preserve.

## Shape 1: basic shallow copy — original unchanged when copy mutates.
a = [1, 2, 3]
b = a.dup
b << 4
puts "orig=#{a.inspect}"
puts "copy=#{b.inspect}"

## Shape 2: object identity — `dup` returns a fresh Array, NOT
## the same object.
puts "identity=#{a.dup.equal?(a)}"

## Shape 3: shallow copy semantics — nested mutable objects
## are shared between original and copy.
inner = "hello"
c = [inner, 1, 2]
d = c.dup
d[0] << " world"
puts "shared-inner=#{c[0].inspect}"
puts "shared-inner-copy=#{d[0].inspect}"

## Shape 4: `clone` behaves the same way at the Tier-1 surface.
e = [10, 20, 30]
f = e.clone
f << 40
puts "clone-orig=#{e.inspect}"
puts "clone-copy=#{f.inspect}"
puts "clone-identity=#{e.clone.equal?(e)}"

## Shape 5: empty array — fresh empty Array, distinct identity.
empty = []
ec = empty.dup
puts "empty-eq=#{empty == ec}"
puts "empty-identity=#{empty.equal?(ec)}"

## Shape 6: sinatra's idiom — snapshot, mutate via side-effect,
## restore. The `dup` boundary is what makes the restore
## meaningful.
class RouteRegistry
  def initialize
    @conditions = []
  end

  def with_snapshot_around(*adds)
    snapshot = @conditions.dup
    adds.each { |x| @conditions << x }
    result = @conditions.dup
    @conditions = snapshot
    result
  end

  def current
    @conditions.dup
  end
end

r = RouteRegistry.new
puts "snap=#{r.with_snapshot_around(:a, :b).inspect}"
puts "after-snap=#{r.current.inspect}"

## Shape 7: `respond_to?` advertises both methods. Sinatra
## doesn't read this but feature-detection should agree with
## dispatch.
puts "respond-dup=#{[].respond_to?(:dup)}"
puts "respond-clone=#{[].respond_to?(:clone)}"
