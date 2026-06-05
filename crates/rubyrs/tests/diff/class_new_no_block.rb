# `Class.new` / `Class.new(superclass)` — no-block path
# returns a real anonymous Class. Pre-fix this fell through
# to the generic Class allocator, which produced a
# `Value::Object` (Instance with `class == Class`) — NOT a
# real `Value::Class`. Downstream `Class.new(anon) { ... }`
# block-form on that "fake-Class" object then tripped
# `TypeError: superclass must be a Class (Object given)`.
#
# P3 Sinatra spike — Mustermann's
# `mustermann/ast/translator.rb:75` reads
#   Class.new(const_get(:NodeTranslator)) do ... end
# where NodeTranslator was minted by an earlier
# `Class.new(Delegator)`; the bad inner Class.new sabotaged
# the outer block-form call.

# 1. `Class.new(Parent)` returns a Class whose superclass is
# Parent and whose class is Class (not Object).
class P1
end
a1 = Class.new(P1)
puts "class=#{a1.class}"
puts "superclass=#{a1.superclass}"

# 2. `Class.new` with no parent defaults to Object (CRuby
# documented).
a2 = Class.new
puts "default_super=#{a2.superclass}"

# 3. The result is `is_a?(Class)` AND `is_a?(Module)` (Class
# subclasses Module in CRuby).
class P3; end
a3 = Class.new(P3)
puts "isa_class=#{a3.is_a?(Class)}"
puts "isa_module=#{a3.is_a?(Module)}"

# 4. Subclassing the returned anonymous Class works — its
# `class` is Class, so `.new.class.superclass` is the same
# anonymous Class.
class P4; end
anon = Class.new(P4)
class C4 < P4; end       # baseline: source-form subclass works
puts "source_subclass=#{C4.superclass}"
# Use the anon class as a real superclass:
sub_anon = Class.new(anon)
puts "anon_subclass_super=#{sub_anon.superclass.class}"

# 5. The block-form `Class.new(anon) do ... end` — the
# specific Mustermann shape this PR unblocks.
class P5; end
anon_super = Class.new(P5)
result = Class.new(anon_super) do
  # Body runs as a class_eval — `self` is the new class.
  # Use a tail expression so the eval has something to
  # produce (matches Mustermann's "build helpers in the
  # body" pattern).
  define_method(:greet) { "hi" }
end
puts "block_result_class=#{result.class}"
puts "block_result_instance_greet=#{result.new.greet}"

# 6. Instances of the anon class allocate as Value::Object
# with the right class identity.
class P6; end
anon = Class.new(P6)
inst = anon.new
# `inst.class` would render `#<Class:0xHEX>` on CRuby vs
# `#<Class>` on rubyrs (anon classes have empty `name` here
# — documented divergence in SUBSET.md). Compare by identity
# via `.equal?` instead.
puts "inst_class_is_anon=#{inst.class.equal?(anon)}"
puts "inst_is_a_anon=#{inst.is_a?(anon)}"
puts "inst_is_a_p6=#{inst.is_a?(P6)}"

# 7. Module given as superclass raises TypeError.
M7 = Module.new
begin
  Class.new(M7)
rescue TypeError => e
  puts "mod_msg_has=#{e.message.include?('superclass must be a Class')}"
end

# 8. Non-Class non-Module raises TypeError with the
# offender's type name.
begin
  Class.new("not a class")
rescue TypeError => e
  puts "scalar_msg_has=#{e.message.include?('String given')}"
end
