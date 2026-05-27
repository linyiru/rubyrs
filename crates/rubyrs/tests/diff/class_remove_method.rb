# `Module#remove_method` — strips the method(s) from this
# class's own methods table. Does NOT walk the superclass chain
# (that's `undef_method`'s job in CRuby; we route undef as a
# no-op pending real semantics).
#
# Motivating consumer: tilt-2.7.0 `lib/tilt/template.rb:490`
#
#   TOPOBJECT.class_eval { remove_method(method_name) }
#
# called after each `evaluate` to wipe the synthesised
# `__tilt_<id>` method from `Tilt::CompiledTemplates`. Without
# this arm tilt's cleanup path raises NoMethodError on every
# render.
#
# Coverage:
#   - Single Symbol arg: removes the entry, returns the class
#   - String arg: same (CRuby `to_sym`'s it)
#   - Variadic: accepts multiple symbols in one call
#   - Missing method on user class raises NameError
#   - Primitive class ALSO raises NameError on missing entries
#     (CRuby parity — DIFFERENT stance from `instance_method` /
#     `method_defined?` which are permissive there)
#   - 0-arg shape: no-op, returns receiver
#   - `respond_to?(:remove_method)` advertises the method

# --- Single Symbol arg: removes, returns class ---
class A
  def hello; "hello"; end
end
puts A.new.hello                         # hello
ret = A.remove_method(:hello)
puts ret.class                           # Class
begin
  A.new.hello
rescue NoMethodError
  puts "hello → NoMethodError after remove"
end

# --- String arg: same (CRuby to_sym's it) ---
class B
  def world; "world"; end
end
B.remove_method("world")
begin
  B.new.world
rescue NoMethodError
  puts "world(str) → NoMethodError after remove"
end

# --- Variadic ---
class C
  def x; "x"; end
  def y; "y"; end
end
C.remove_method(:x, :y)
begin
  C.new.x
rescue NoMethodError
  puts "x → gone"
end
begin
  C.new.y
rescue NoMethodError
  puts "y → gone"
end

# --- Missing method on user class raises NameError ---
class D
end
begin
  D.remove_method(:nonexistent)
rescue NameError
  puts "missing → NameError"
end

# --- Primitive class also raises NameError on missing entries
#     (CRuby parity — UNLIKE `instance_method` / `method_defined?`
#     which are permissive). `remove_method` is an actual
#     mutation, not a probe; surfacing the missing-entry shape
#     loudly here matches CRuby and avoids quiet divergence.
begin
  String.remove_method(:nonexistent_xyz)
rescue NameError
  puts "primitive missing → NameError"
end

# --- Non-Symbol-non-String arg raises TypeError
#     "<inspect> is not a symbol nor a string" (CRuby parity).
class TypeCheck
end
begin
  TypeCheck.remove_method(123)
rescue TypeError
  puts "int → TypeError"
end
begin
  TypeCheck.remove_method(nil)
rescue TypeError
  puts "nil → TypeError"
end

# --- 0-arg shape: no-op, returns receiver ---
class E
end
puts E.remove_method.equal?(E)           # true

# --- respond_to? whitelist consistency ---
puts A.respond_to?(:remove_method)       # true
