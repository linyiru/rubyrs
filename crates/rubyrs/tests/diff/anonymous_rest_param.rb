## Anonymous rest parameter `def foo(*)` — Ruby 2.0+ forwarding
## form. Before this layer's fix, rubyrs's translator dropped the
## rest slot when `RestParameterNode#name == None`, so the method
## compiled with arity 0 and rejected ANY positional arg.
##
## Discovery context: TRY_RUNS pass-11 probe — sinatra-4 stubs
## `def self.new(*)` on Mustermann; the real `Mustermann.new(path,
## **opts)` at sinatra/base.rb:1818 then raised "wrong number of
## arguments (given 2, expected 0)". (Layer #13.)

## Shape 1: anonymous `*` accepts arbitrary positional args.
def shape1(*); :ok; end
puts "shape1-zero=#{shape1}"
puts "shape1-one=#{shape1(1)}"
puts "shape1-many=#{shape1(1, 2, 3)}"

## Shape 2: required + anonymous `*` — required still enforced,
## extras absorbed.
def shape2(a, *); a; end
puts "shape2-min=#{shape2(:x)}"
puts "shape2-extra=#{shape2(:x, :y, :z)}"

## Shape 3: anonymous `**` (Ruby 3.1+ anonymous kwsplat).
def shape3(**); :kw; end
puts "shape3-zero=#{shape3}"
puts "shape3-kw=#{shape3(a: 1, b: 2)}"

## Shape 4: anonymous `&` (Ruby 3.1+ anonymous block fwd).
def shape4(&); :blk; end
puts "shape4-none=#{shape4}"
puts "shape4-block=#{shape4 { 1 }}"

## Shape 5: introspection — Method#parameters / #arity match
## CRuby's `[[:rest, :*]]` shape for anonymous rest.
class C
  def f(*); end
  def g(a, *); end
end
m = C.new.method(:f)
puts "shape5-params=#{m.parameters.inspect}"
puts "shape5-arity=#{m.arity}"
m2 = C.new.method(:g)
puts "shape5b-params=#{m2.parameters.inspect}"
puts "shape5b-arity=#{m2.arity}"

## Shape 6: singleton-method `def self.new(*)` — the exact
## sinatra-stub shape that triggered the layer-#13 probe.
class D
  def self.new(*); :allocated; end
end
puts "shape6-zero=#{D.new}"
puts "shape6-many=#{D.new(1, 2, 3)}"
