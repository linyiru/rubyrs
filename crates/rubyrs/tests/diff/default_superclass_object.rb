# `class Foo; end` with no explicit parent defaults to Object,
# matching CRuby. Before this fix, the default was nil and
# `Object === Foo.new` returned false — which silently broke
# tilt's render dispatch (template.rb:257 `case scope when Object`
# fell through to the BasicObject fallback that needs
# `Kernel.instance_method(:class)`).

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
# superclass chain walk: Bar → Foo → Object → BasicObject
puts Bar.superclass.superclass               # Object
puts Bar.superclass.superclass.superclass    # BasicObject

# --- case/when on Object catches user-class instances ---
# (the real-world unblock — tilt scope dispatch)
result = case Foo.new
         when Object then "matched Object"
         else "fell through"
         end
puts result                                  # matched Object

# --- Modules don't have a superclass. Both CRuby and rubyrs
#     now raise NoMethodError on `Module#superclass`. ---
module M
end
puts M.is_a?(Module)                         # true
puts M.is_a?(Class)                          # false
begin
  M.superclass
  puts "no raise (BAD)"
rescue NoMethodError
  puts "Module.superclass raises NoMethodError"
end

# --- Reopen doesn't re-default the chain ---
class Foo
end
puts Foo.superclass                          # still Object
