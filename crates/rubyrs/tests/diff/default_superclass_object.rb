# `class Foo; end` with no explicit parent defaults to Object,
# matching CRuby. Before this fix, the default was nil and
# `Object === Foo.new` returned false — which silently broke
# tilt's render dispatch (template.rb:257 `case scope when Object`
# fell through to the BasicObject fallback that needs
# `Kernel.instance_method(:class)`).
#
# We don't model BasicObject / Kernel, so the fixture avoids
# `ancestors` (whose CRuby output would include both) and uses
# `is_a?` / `===` / `superclass` chain walking instead — those
# observe the relationship without requiring the full chain.

# --- Default superclass is Object ---
class Foo
end
puts Foo.superclass                          # Object
puts Foo.new.is_a?(Object)                   # true
puts Object === Foo.new                      # true

# --- Explicit parent unchanged ---
class Bar < Foo
end
puts Bar.superclass                          # Foo
puts Bar.new.is_a?(Foo)                      # true
puts Bar.new.is_a?(Object)                   # true (transitive)
# superclass chain walk: Bar → Foo → Object
puts Bar.superclass.superclass               # Object

# --- case/when on Object catches user-class instances ---
# (the real-world unblock — tilt scope dispatch)
result = case Foo.new
         when Object then "matched Object"
         else "fell through"
         end
puts result                                  # matched Object

# --- Modules don't get Object as superclass. CRuby raises
#     NoMethodError on `Module#superclass`; rubyrs returns nil
#     (documented divergence). The important parity here is that
#     `module M; end` doesn't silently get Object grafted onto
#     its (non-existent) chain by the new default — verify
#     `is_a?(Module)` works without an Object detour. ---
module M
end
puts M.is_a?(Module)                         # true
puts M.is_a?(Class)                          # false

# --- Reopen doesn't re-default the chain ---
class Foo
end
puts Foo.superclass                          # still Object
