# `respond_to?` must see methods REOPENED onto a core class (and methods
# inherited from a module the class includes), not just the hardcoded
# builtin primitive list. Previously `[].respond_to?(:lazy)` (a preamble
# method) and any `class Array; def custom; end` reported false even
# though the call dispatches fine.

# preamble methods reopened on builtins
p [].respond_to?(:lazy)
p({}.respond_to?(:lazy))
p((1..3).respond_to?(:lazy))

# builtin primitives still report true
p [].respond_to?(:map)
p "x".respond_to?(:upcase)
p 1.respond_to?(:+)

# user reopening of a core class
class Array
  def my_custom_thing; 42; end
end
p [].respond_to?(:my_custom_thing)
p [1].my_custom_thing

class String
  def shout; upcase + "!"; end
end
p "hi".respond_to?(:shout)
p "hi".shout

# absent methods still report false
p [].respond_to?(:totally_not_a_method)
p 1.respond_to?(:nonexistent_xyz)

# user class respond_to? unaffected
class Widget
  def gear; end
end
p Widget.new.respond_to?(:gear)
p Widget.new.respond_to?(:missing)
