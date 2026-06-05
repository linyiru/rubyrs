# `Module#const_set(name, value)` — install a constant on a
# class. CRuby returns the assigned value; subsequent reads
# (`Foo::BAR`, `Foo.const_get(:BAR)`) see it.
#
# P3 Sinatra spike — Mustermann's
# `mustermann/ast/translator.rb:62` calls
#   subclass.const_set(:NodeTranslator, node_translator)
# inside the `inherited` hook. Pre-fix this hit
# `NoMethodError: undefined method 'const_set' for Class`.

# 1. Basic value install + readback through `Foo::CONST`.
class C1
end
C1.const_set(:NUM, 42)
puts "by_path=#{C1::NUM}"

# 2. Symbol vs String name accept.
class C2
end
C2.const_set(:SYM_NAME, "via-sym")
C2.const_set("STR_NAME", "via-string")
puts "sym=#{C2::SYM_NAME}"
puts "str=#{C2::STR_NAME}"

# 3. Returns the assigned value (CRuby semantics).
class C3
end
ret = C3.const_set(:THING, :payload)
puts "ret=#{ret.inspect}"

# 4. Installed Class is itself subclassable + new-able.
class C4
end
C4.const_set(:Inner, Class.new { def greet; "hi"; end })
puts "inner_class=#{C4::Inner.class}"
puts "inner_instance=#{C4::Inner.new.greet}"

# 5. `const_get` round-trip — the just-set constant is
# readable.
class C5
end
C5.const_set(:RT, "round-trip")
puts "via_get=#{C5.const_get(:RT)}"

# 6. Lowercase-leading names raise NameError (CRuby parity —
# Ruby constants must start with uppercase).
class C6
end
begin
  C6.const_set(:lower, 1)
rescue NameError => e
  puts "lower_msg_has=#{e.message.include?('wrong constant name')}"
end

# 7. The mustermann shape: `inherited` hook sets a constant
# on the subclass at class-build time.
class Base7
  def self.inherited(sub)
    nt = Class.new
    sub.const_set(:NodeTranslator, nt)
  end
end
class Child7 < Base7; end
puts "inherited_set_class=#{Child7::NodeTranslator.class}"

# 8. Overwriting a constant — CRuby emits a warning on
# stderr but allows the write; rubyrs's stderr capture in
# the diff harness ignores stderr, so byte-identical stdout
# is the contract.
class C8
end
C8.const_set(:V, 1)
C8.const_set(:V, 2)
puts "overwritten=#{C8::V}"
