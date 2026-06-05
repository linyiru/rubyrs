# Anonymous-class `const_set` must NOT collide with toplevel
# or with other anon classes. Pre-fix, anon `cls.const_set
# (:X, v)` built a qualified key (`format!("{}::{}", "", "X")`
# → `"::X"`) that aliased the toplevel `X` constant key and
# `self.classes.insert(key, ...)` clobbered the toplevel
# class registration as a side effect — a catastrophic
# unstated cross-scope leak. Per-class storage via
# `Class::consts: RefCell<HashMap<SymId, Value>>` isolates
# each anon class's writes.
#
# Surfaced during the code-review sweep on the
# Sinatra-spike commit chain.

# 1. Toplevel constant unchanged after anon const_set on
# the same name.
TOPLEVEL_C = "toplevel-val"
a = Class.new
a.const_set(:TOPLEVEL_C, "anon-scoped")
puts "toplevel_after=#{TOPLEVEL_C}"
puts "anon_get=#{a.const_get(:TOPLEVEL_C)}"

# 2. Different anon classes have isolated per-class const
# tables — setting the same name on each yields different
# values, no overwrite.
a1 = Class.new
a2 = Class.new
a1.const_set(:X, "from-a1")
a2.const_set(:X, "from-a2")
puts "a1_X=#{a1.const_get(:X)}"
puts "a2_X=#{a2.const_get(:X)}"

# 3. Anon-class const_set with a Class value doesn't pollute
# the global class registry: `Foo.new` on a SEPARATE
# top-level `Foo` continues to allocate Foo-instances, not
# the anon's Foo.
class Foo
  def label; "real-Foo"; end
end
b = Class.new
b.const_set(:Foo, Class.new { def label; "anon-Foo"; end })
puts "tl_foo=#{Foo.new.label}"
puts "anon_foo=#{b.const_get(:Foo).new.label}"

# 4. Returning the assigned value — same CRuby semantics
# `const_set` honoured here too.
c = Class.new
ret = c.const_set(:RET, :payload)
puts "ret=#{ret.inspect}"

# 5. Lowercase still raises NameError on anon receivers.
d = Class.new
begin
  d.const_set(:lower, 1)
rescue NameError => e
  puts "lower_anon=#{e.message.include?('wrong constant name')}"
end
