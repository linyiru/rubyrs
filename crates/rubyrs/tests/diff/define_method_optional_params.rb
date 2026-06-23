# A `define_method` body honours OPTIONAL parameters (defaults), like a
# real method — not strict arity. dry-core's ClassAttributes builds
# `define_method(name) { |value = Undefined| ... }` getters/setters this
# way (dry-monads, dry-configurable, …).
class C
  define_method(:f0) { |a = 1| a }
  define_method(:f1) { |a, b = 2| [a, b] }
  define_method(:f2) { |a, b = 2, *c| [a, b, c] }
end
o = C.new
p o.f0; p o.f0(9)
p o.f1(1); p o.f1(1, 20)
p o.f2(1); p o.f2(1, 2, 3, 4)
p((o.f1 rescue $!.message))           # given 0, expected 1..2
p((o.f1(1, 2, 3) rescue $!.message))  # given 3, expected 1..2
p((o.f0(1, 2) rescue $!.message))     # given 2, expected 0..1
