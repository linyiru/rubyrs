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
puts "get-str=#{b.instance_variable_get("@value")}"

## Reading an undefined ivar returns nil (CRuby semantics:
## no warning, no NameError).
puts "get-undef=#{b.instance_variable_get(:@nope).inspect}"

## Write by Symbol — returns the assigned value.
ret = b.instance_variable_set(:@value, "changed")
puts "set-ret=#{ret}"
puts "direct-after-set=#{b.value}"

## Write by String.
b.instance_variable_set("@value", 99)
puts "direct-after-str-set=#{b.value}"

## Write a new (previously-unset) ivar — succeeds; subsequent
## get returns the stored value.
b.instance_variable_set(:@newcomer, "hello")
puts "newcomer=#{b.instance_variable_get(:@newcomer)}"

## Name validation: a Symbol/String without `@` prefix is
## rejected with NameError (CRuby raises
## `NameError: '<name>' is not allowed as an instance variable name`).
begin
  b.instance_variable_get(:foo)
  puts "no-prefix-get=NOT-RAISED"
rescue NameError => e
  puts "no-prefix-get=#{e.class}"
end
begin
  b.instance_variable_set(:foo, 1)
  puts "no-prefix-set=NOT-RAISED"
rescue NameError => e
  puts "no-prefix-set=#{e.class}"
end

## Type validation: non-Symbol-non-String args raise TypeError.
begin
  b.instance_variable_get(123)
  puts "wrong-type-get=NOT-RAISED"
rescue TypeError => e
  puts "wrong-type-get=#{e.class}"
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
