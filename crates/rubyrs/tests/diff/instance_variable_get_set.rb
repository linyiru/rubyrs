## Object#instance_variable_get / #instance_variable_set —
## CRuby-compatible introspection surface for reading/writing
## ivars by name. Surfaced as a real gap by TRY_RUNS pass 7
## layer #2 (sinatra/indifferent_hash.rb's Gem::Version#<=>
## shape uses `o.instance_variable_get(:@s)` to compare across
## opaque receivers).

class Box
  def initialize(x); @value = x; end
  def value; @value; end
end

b = Box.new(42)
puts "direct=#{b.value}"

## Read by Symbol.
puts "get-sym=#{b.instance_variable_get(:@value)}"

## Read by String — same result.
puts "get-str=#{b.instance_variable_get('@value')}"

## Reading an undefined ivar returns nil (CRuby semantics:
## no warning, no NameError).
puts "get-undef=#{b.instance_variable_get(:@nope).inspect}"

## Write by Symbol — returns the assigned value.
ret = b.instance_variable_set(:@value, "changed")
puts "set-ret=#{ret}"
puts "direct-after-set=#{b.value}"

## Write by String.
b.instance_variable_set('@value', 99)
puts "direct-after-str-set=#{b.value}"

## Write a new (previously-unset) ivar — succeeds; subsequent
## get returns the stored value.
b.instance_variable_set(:@newcomer, "hello")
puts "newcomer=#{b.instance_variable_get(:@newcomer)}"

## Name validation: a Symbol/String without `@` prefix is
## rejected with NameError. Pin class + message so a change to
## CRuby's text format (or a divergence on rubyrs's side) trips
## the diff.
begin
  b.instance_variable_get(:foo)
  puts "no-prefix-get=NOT-RAISED"
rescue NameError => e
  puts "no-prefix-get=#{e.class}: #{e.message}"
end
begin
  b.instance_variable_set(:foo, 1)
  puts "no-prefix-set=NOT-RAISED"
rescue NameError => e
  puts "no-prefix-set=#{e.class}: #{e.message}"
end

## Type validation: non-Symbol-non-String args raise TypeError
## on BOTH get and set paths. CRuby reports exact message
## "<inspect> is not a symbol nor a string"; pin both class and
## message so a change in either side surfaces in the diff.
begin
  b.instance_variable_get(123)
  puts "wrong-type-get=NOT-RAISED"
rescue TypeError => e
  puts "wrong-type-get=#{e.class}: #{e.message}"
end
begin
  b.instance_variable_set(123, "x")
  puts "wrong-type-set=NOT-RAISED"
rescue TypeError => e
  puts "wrong-type-set=#{e.class}: #{e.message}"
end

## Name validation: CRuby ivar names must match
## `@[A-Za-z_][A-Za-z0-9_]*`. The "starts with @" guard alone
## isn't enough — names like `@@x` (class var), `@1foo`,
## `@foo?`, bare `@` all need to be rejected.
## Probe each shape via its String form so the test output
## doesn't depend on `Symbol#inspect`'s quoting rules for
## non-identifier symbols (CRuby wraps these in quotes;
## rubyrs doesn't yet — out of scope for this PR).
[
  "@@klass_var",   # class-variable shape (double @)
  "@1foo",         # digit start after @
  "@foo?",         # predicate suffix not legal for ivars
  "@foo=",         # writer suffix not legal for ivars
  "@",             # bare @ with no body
].each do |bad|
  begin
    b.instance_variable_get(bad)
    puts "bad-name-get(#{bad})=NOT-RAISED"
  rescue NameError => e
    puts "bad-name-get(#{bad})=#{e.class}: #{e.message}"
  end
end

## Class receivers carry their own ivar table (mirror of
## `Op::LoadIvar` / `Op::StoreIvar` in vm/step.rs); both
## get and set should reach it. This is what CRuby does for
## `MyClass.instance_variable_get(:@registry)` (a common
## pattern for class-level state).
class Holder
  @registry = "boot"
end
puts "class-get=#{Holder.instance_variable_get(:@registry)}"
Holder.instance_variable_set(:@registry, "updated")
puts "class-get-after=#{Holder.instance_variable_get(:@registry)}"

## respond_to? must agree with dispatch: every value responds
## to instance_variable_get / instance_variable_set even if the
## result is uninteresting (primitives) or raises (set on
## primitives). Without the universal-method whitelist update
## in vm/lookup.rs, respond_to? would lie about this.
[42, "hi", :sym, [1], {a: 1}].each do |v|
  puts "respond_to-#{v.class}-get=#{v.respond_to?(:instance_variable_get)}"
  puts "respond_to-#{v.class}-set=#{v.respond_to?(:instance_variable_set)}"
end

## Primitive-receiver semantics. respond_to? returning true
## (above) is only meaningful if the actual call behaves
## as documented: get returns nil; set raises FrozenError.
## Exercise an Integer receiver explicitly so a regression
## that ICE's (or silently returns the wrong shape) trips
## here instead of much later in a downstream gem.
puts "int-get=#{42.instance_variable_get(:@x).inspect}"
begin
  42.instance_variable_set(:@x, 1)
  puts "int-set=NOT-RAISED"
rescue FrozenError => e
  puts "int-set=#{e.class}: #{e.message}"
end

## Wrong-arity: CRuby raises ArgumentError with the standard
## "wrong number of arguments (given N, expected M)" shape.
## Without the explicit arity arm, this would fall through to
## NoMethodError (semantically wrong).
begin
  b.instance_variable_get
  puts "arity-get-0=NOT-RAISED"
rescue ArgumentError => e
  puts "arity-get-0=#{e.class}"
end
begin
  b.instance_variable_set(:@x)
  puts "arity-set-1=NOT-RAISED"
rescue ArgumentError => e
  puts "arity-set-1=#{e.class}"
end

## End-to-end: the sinatra-shaped Gem::Version#<=> usage that
## surfaced this gap in TRY_RUNS pass 7 layer #2.
module Gem
  class Version
    include Comparable
    def initialize(s); @s = s.to_s; end
    def <=>(o); @s <=> o.instance_variable_get(:@s); end
    def to_s; @s; end
  end
end

a = Gem::Version.new("3.4.1")
b = Gem::Version.new("3.0")
puts "ge=#{a >= b}"
puts "lt=#{a < b}"
puts "eq=#{a == b}"
