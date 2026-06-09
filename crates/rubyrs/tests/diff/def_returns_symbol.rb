# `def name` evaluates to the method name Symbol (CRuby), which is what
# makes the `private def …` / `public def …` / `module_function def …`
# modifier idiom work — the modifier receives `:name`.

p(def foo; end)                 # :foo
x = def bar; 1; end
p x                             # :bar

class C
  p(def imeth; end)             # :imeth
  p(def self.cmeth; end)        # :cmeth

  private def secret; 42; end
  public def shown; 1; end
end

# private def actually made it private
p C.new.respond_to?(:secret)        # false
p C.new.respond_to?(:secret, true)  # true
p C.new.respond_to?(:shown)         # true
begin
  C.new.secret
rescue NoMethodError
  p :secret_is_private
end

# def on a singleton
o = Object.new
p(def o.only_mine; 7; end)      # :only_mine
p o.only_mine                   # 7

# module_function def
module M
  module_function def helper; "h"; end
end
p M.helper                      # "h"
