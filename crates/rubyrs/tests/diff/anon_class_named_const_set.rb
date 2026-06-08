# CRuby names an anonymous class/module on its FIRST
# constant-assignment, and the constants it accrued while
# anonymous (via implicit-self `const_set` inside a
# `Class.new { ... }` block) must then be reachable through
# the qualified read path. This is the rouge token-tree shape
# (token.rb builds `Class.new(parent){ const_set(:Sub, ...) }`
# nested several levels deep, then references the leaves by a
# deep qualified name).
#
# Pre-fix: anon `const_set` wrote to the per-class `consts`
# table but the class was never named on assignment and never
# registered in the global class map, so `C::X` raised
# "uninitialized constant C::X (NameError)" and `C.name` was
# nil.

# 1. const_set inside a Class.new block, then qualified read.
C = Class.new { const_set(:X, 42) }
p C.name
p C::X

# 2. Anon-class naming on first const-assignment.
Anon = Class.new
p Anon.name
M = Module.new
p M.name

# 3. const_set then bare-name read inside a later method.
# (The method is added AFTER assignment so its lexical scope
#  is the now-named class — CRuby resolves the bare const here.)
class WithConst; end
WithConst.const_set(:GREETING, "hi")
class WithConst
  def self.fetch; GREETING; end
end
p WithConst.fetch
p WithConst::GREETING

# 4. Nested const_set creating a sub-class, read both ways.
D = Class.new do
  const_set(:Inner, Class.new)
  const_set(:Y, 7)
end
p D::Y
p D::Inner.class
p D::Inner.name

# 5. Deep nesting — const_set on an anon class that itself
# holds an anon class with its own const_set.
E = Class.new do
  const_set(:Mid, Class.new { const_set(:Leaf, 99) })
end
p E::Mid::Leaf
p E::Mid.name
p E::Mid.class

# 6. const_set on a NAMED class read back (qualified + bare).
class Named; end
Named.const_set(:K, 5)
p Named::K

# 7. First-assignment-wins: a later alias does NOT rename.
Orig = Class.new
Alias = Orig
p Orig.name
p Alias.name

# 8. NameError must STILL raise for a never-set const.
begin
  p C::NEVER_SET
rescue NameError => e
  puts e.message
end
begin
  p D::Inner::ALSO_MISSING
rescue NameError => e
  puts e.message
end
