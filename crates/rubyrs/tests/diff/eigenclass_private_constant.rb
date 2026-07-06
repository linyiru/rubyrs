## S3 item (b): `private_constant` inside an eigenclass body is
## enforced. CRuby semantics (probed 3.4.8):
##   - lexical reads from the body's methods still work;
##   - the `::` reference form raises
##     "private constant #<Class:X>::NAME referenced";
##   - `const_get` deliberately BYPASSES constant privacy;
##   - `const_defined?` stays true; `constants(false)` hides it.
##
## The `::`-vs-const_get split rides the new Op::LoadConstFromValue
## (dynamic-base constant read): the AST used to desugar `expr::CONST`
## straight to `expr.const_get(:CONST)`, which silently bypassed
## privacy for EVERY dynamic base — also fixed here for normal
## modules (`m::PRIV` below).

class Widget
  class << self
    SECRET = "hidden"
    private_constant :SECRET
    def peek
      SECRET
    end
  end
end

puts "peek=#{Widget.peek}"

sc = Widget.singleton_class
begin
  sc::SECRET
  puts "colon2=NOT-ENFORCED"
rescue NameError => e
  puts "colon2=#{e.message}"
end

## const_get bypasses privacy (CRuby).
puts "const_get=#{sc.const_get(:SECRET)}"
puts "const_defined=#{sc.const_defined?(:SECRET)}"
puts "constants=#{sc.constants(false).inspect}"

## public_constant re-exposes.
class Widget
  class << self
    public_constant :SECRET
  end
end
puts "reexposed=#{sc::SECRET}"
puts "relisted=#{sc.constants(false).inspect}"

## The same enforcement through a DYNAMIC base on a normal module —
## the adjacent pre-existing gap the colon2 op closes.
module Vault
  PRIV = 7
  private_constant :PRIV
end
holder = Vault
begin
  holder::PRIV
  puts "dyn=NOT-ENFORCED"
rescue NameError => e
  puts "dyn=#{e.message}"
end
puts "dyn_const_get=#{holder.const_get(:PRIV)}"

## A non-class dynamic base raises CRuby's TypeError (the old
## const_get desugar surfaced a confusing NoMethodError).
n = 5
begin
  n::FOO
  puts "nonclass=NO-ERROR"
rescue TypeError => e
  puts "nonclass=#{e.class}: #{e.message}"
end
