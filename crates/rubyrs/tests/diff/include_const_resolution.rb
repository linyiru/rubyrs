# `include SomeModule` brings the module's constants into the
# includer's constant-resolution scope (CRuby's step-2 ancestor
# search). This is THE behaviour rouge's ~240 lexers rely on:
# `include Token::Tokens` then bare `Text` / `Str::Double`.
#
# Pins both halves of constant resolution through an included
# module:
#   - bare-name reads inside a method (LoadConstChain path)
#   - qualified `C::CONST` reads (LoadConst path)

module M
  FOO = 42
  class Bar; end
  module Str
    Double = "M::Str::Double"
  end
end

class C
  include M
  def get = FOO          # bare const from included M
  def get_bar = Bar      # bare class from included M
  def get_double = Str::Double  # nested bare const through include
end

p C.new.get              # 42
p C.new.get_bar          # M::Bar
p C.new.get_double       # "M::Str::Double"
p C::FOO                 # 42  (qualified, through include)
p C::Bar                 # M::Bar (qualified class, through include)
p C::Str::Double         # "M::Str::Double"
