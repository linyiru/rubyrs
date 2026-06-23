# Module#class_variable_get / _set / _defined? + class_variables —
# cvar reflection (shared across the ancestor chain). ActiveSupport's
# mattr_accessor uses class_variable_set.
class C
  @@x = 1
end
p C.class_variable_get(:@@x)
p C.class_variable_set(:@@y, 2)
p C.class_variable_get(:@@y)
p C.class_variable_defined?(:@@x)
p C.class_variable_defined?(:@@z)
p C.class_variables.sort

class D < C
  @@d = 9
end
p D.class_variable_get(:@@x)          # inherited
p D.class_variable_defined?(:@@x)     # inherited
p D.class_variables(false)            # own only
p D.class_variable_set(:@@x, 100)     # writes the owning ancestor
p C.class_variable_get(:@@x)          # 100

# string name form
p C.class_variable_get("@@y")

begin; C.class_variable_get(:@@nope); rescue => e; puts "#{e.class}: #{e.message}"; end
begin; C.class_variable_get(:x); rescue => e; puts "#{e.class}: #{e.message}"; end
