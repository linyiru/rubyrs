# Top-level `self` is the `main` object (a singleton Object), like CRuby
# — not nil. So `self.extend Module` works at the top level, and `main`
# renders as "main". (rake/dsl_definition.rb:196 `self.extend Rake::DSL`.)
p self.class                 # Object
p self.is_a?(Object)         # true
p self.to_s                  # "main"
p self.inspect               # "main"
p self.respond_to?(:extend)  # true

# self.extend adds the module's methods to main (top-level bare calls
# then dispatch to them — how rake's DSL becomes available).
module Greeter
  def greet(n); "hi #{n}"; end
end
self.extend(Greeter)
p greet("rake")              # "hi rake" (bare call dispatches via main)

# A top-level `def` still wins over a same-named Kernel method (CRuby:
# top-level defs are private methods on Object, ahead of Kernel).
module Kernel
  def widget(x); x * 2; end
end
p widget(5)                  # 10 (Kernel, no top-level def yet)
def widget(x); x * 100; end
p widget(5)                  # 500 (top-level def wins)
