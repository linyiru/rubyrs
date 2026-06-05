# `const_get` / bare-const lookup walks the calling class's
# inheritance chain. Pre-fix, `resolve_const_path` only tried
# `scope_name::segment` (single direct lookup); an anonymous
# subclass (`Class.new(Parent) do ... end`) had scope_name == ""
# which produced the malformed lookup `"::segment"` and missed
# every constant the parent class had defined.
#
# P3 Sinatra spike — Mustermann's
# `mustermann/ast/translator.rb:75` reads
#   Class.new(const_get(:NodeTranslator)) do ... end
# from inside a method body running on an anonymous subclass.
# Pre-fix tripped `NameError: uninitialized constant
# ::NodeTranslator`.

# 1. Named subclass inheriting a parent's constant — direct
# `Sub.const_get(:X)` succeeds when X is defined on Parent.
class P1
  C1 = "from-P1"
end
class S1 < P1
end
puts "named_sub=#{S1.const_get(:C1)}"

# 2. Implicit-receiver `const_get` from inside a method
# defined on the parent — self is the subclass, lookup walks
# parent's scope.
class P2
  C2 = "from-P2-scope"
  def self.lookup; const_get(:C2); end
end
class S2 < P2; end
puts "method_implicit=#{S2.lookup}"

# 3. The Mustermann shape: implicit const_get from inside a
# block running on an anonymous Class.new(Parent) class.
class P3
  C3 = "via-anon-subclass"
  def self.translate
    Class.new(self) do
      # `const_get` here runs in class_eval context — self is
      # the new anon class, scope walks to Parent.
      const_get(:C3)
    end
  end
end
result = P3.translate
puts "anon_via_class_eval=#{result.superclass == P3}"

# 4. Multi-level inheritance — Grandchild → Child → Parent
# chain walks all the way up.
class GP4
  CONST = "from-grandparent"
end
class P4 < GP4; end
class C4 < P4; end
puts "multi_level=#{C4.const_get(:CONST)}"

# 5. Constant defined on Subclass takes precedence over
# parent (shadowing).
class P5
  V = "parent"
end
class S5 < P5
  V = "child"
end
puts "shadow=#{S5.const_get(:V)}"

# 6. Toplevel fallback — if neither the receiver nor its
# inheritance chain defines the constant, fall through to
# toplevel (Object scope). CRuby parity.
TOPLEVEL_C6 = "tl"
class Iso6
end
puts "toplevel_fallback=#{Iso6.const_get(:TOPLEVEL_C6)}"

# 7. Missing constant → NameError. The error reports the
# qualified-lookup path that was tried, NOT the malformed
# `::Name` shape the anon-class case would have produced
# pre-fix.
class P7; end
begin
  P7.const_get(:NEVER_DEFINED_C7)
rescue NameError => e
  puts "missing_msg_has_name=#{e.message.include?('NEVER_DEFINED_C7')}"
end
