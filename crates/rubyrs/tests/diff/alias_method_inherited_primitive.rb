## `alias_method` walks the superclass chain looking for a
## primitive class that responds to the source method. Pre-fix
## the lookup only checked the immediate class's name against
## the primitive whitelist, so `class P < Hash; alias_method
## :a, :to_h; end` raised `NameError: undefined method 'to_h'
## for class 'P'` (P isn't a primitive class name, even though
## its ancestor Hash is).
##
## Discovery context: rack-3.1.10/lib/rack/query_parser.rb:197
## defines `class Params < Hash; alias_method
## :to_params_hash, :to_h; end`. sinatra-4 transitively
## requires rack/query_parser, so loading `sinatra/base`
## tripped on this. (TRY_RUNS pass-10 layer #11.)

## Shape 1: rack's exact pattern — `class Params < Hash`
## aliasing the inherited `to_h`. Pre-fix `alias_method` raised
## NameError because the primitive-class probe only checked
## `cls.name` ("Params"), not its ancestor chain (where Hash
## sits and is whitelisted). The presence of the alias method
## is the regression signal — we don't need to invoke it
## (the wider Hash-subclass dispatch story is a separate gap).
class Params < Hash
  alias_method :to_params_hash, :to_h
end
puts "alias-defined=#{Params.method_defined?(:to_params_hash)}"
puts "alias-listed=#{Params.instance_methods(false).include?(:to_params_hash)}"

## Shape 2: deeper inheritance chain — primitive ancestor is
## not the immediate superclass.
class HashChild < Hash; end
class GrandChild < HashChild
  alias_method :compact_alias, :compact
end
puts "deep-alias-defined=#{GrandChild.method_defined?(:compact_alias)}"

## Shape 3: Array subclass aliasing a primitive Array method.
class IntArr < Array
  alias_method :first_two, :take
end
puts "array-alias-defined=#{IntArr.method_defined?(:first_two)}"

## Shape 4: String subclass aliasing a primitive String method.
class FancyStr < String
  alias_method :loud, :upcase
end
puts "string-alias-defined=#{FancyStr.method_defined?(:loud)}"

## Shape 5: regression — aliasing a method that exists on the
## class itself still works (no walk needed). Pre-fix the
## immediate-class probe handled this; the walk extension
## must not regress it.
class Direct
  def own_method
    "direct"
  end
  alias_method :own_alias, :own_method
end
puts "direct-alias=#{Direct.new.own_alias}"

## Shape 6: source method genuinely missing — divergence
## documented. rubyrs's `primitive_class_responds_to` for
## Hash/Array/Range falls through to `is_primitive_class_name`
## (a name check), which returns true regardless of whether
## the method actually exists on the primitive. So
## `alias_method :a, :no_such_method` on a Hash subclass
## succeeds at alias time and fails only when invoked. CRuby
## raises NameError immediately. Pre-existing divergence for
## the non-sentinel primitives; pinned by NOT asserting an
## error here so a future fix that adds Hash/Array sentinels
## can land without breaking diff harness expectations.
