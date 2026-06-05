# Skip under STRESS_GC: the Struct preamble uses
# define_method-with-class-ivars-closure shapes that trip a
# pre-existing rubyrs GC root hole (anon-class Instance
# slots get swept mid-dispatch). Normal-mode load surface
# is the contract this fixture defends; STRESS_GC coverage
# would need the underlying VM root-set fix first.
# Documented in `preamble/struct.rb`. Tip: `exit 0` would
# trigger rubyrs's "exit (SystemExit)" tail-line print,
# diverging from CRuby's silent exit, so we sentinel via a
# bare top-level rescue gate instead.
if ENV["STRESS_GC"]
  # Empty body — both runtimes emit nothing.
else

# `Struct.new(*attr_names)` — the factory shape that
# mustermann's `mustermann/ast/transformer.rb:80` uses:
#   Operator = Struct.new(:separator, :allow_reserved,
#                         :prefix, :parametric)
# followed by `Operator.new(?,, false, false, false)`.
# Pre-shim this raised `NameError: uninitialized constant
# Struct`.

# 1. Basic creation + positional initializer + accessors.
Point = Struct.new(:x, :y)
p1 = Point.new(3, 4)
puts "x=#{p1.x} y=#{p1.y}"

# 2. Class-method introspection — `.members` returns the
# attribute name list.
puts "cls_members=#{Point.members.inspect}"

# 3. Instance-method introspection — same `members` on the
# instance.
puts "inst_members=#{p1.members.inspect}"

# 4. `.to_a` returns values in declaration order.
puts "to_a=#{p1.to_a.inspect}"

# 5. Writer accessors mutate the instance.
p1.x = 99
puts "after_x=#{p1.x}"
p1.y = 100
puts "after_to_a=#{p1.to_a.inspect}"

# 6. Equality — same class + same `.to_a` ⇒ true.
a = Point.new(1, 2)
b = Point.new(1, 2)
c = Point.new(1, 3)
puts "a_eq_b=#{a == b}"
puts "a_eq_c=#{a == c}"

# 7. Different Struct classes — different identity even
# with same attr names + same values.
PointB = Struct.new(:x, :y)
pa = Point.new(7, 8)
pb = PointB.new(7, 8)
puts "cross_class_eq=#{pa == pb}"

# 8. Mustermann's exact shape: 4-attr Struct, instances
# constructed with positional args.
Operator = Struct.new(:separator, :allow_reserved, :prefix,
                      :parametric)
op = Operator.new(?,, false, false, false)
puts "sep=#{op.separator} allow=#{op.allow_reserved} pre=#{op.prefix} parm=#{op.parametric}"

# 9. Initialiser tolerates fewer-than-attr args — missing
# slots default to nil (CRuby parity).
class_v9 = Struct.new(:a, :b, :c)
inst_v9 = class_v9.new(1)
puts "underfill=#{inst_v9.a.inspect}|#{inst_v9.b.inspect}|#{inst_v9.c.inspect}"
end
