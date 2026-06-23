# `class << <expr>; ...; end` evaluates to its LAST expression (CRuby),
# and an arbitrary body (not just def/attr/alias) runs as a real
# eigenclass body. rspec-mocks' `(class << object; ancestors; end).map`.
obj = Object.new
def obj.foo; end
p((class << obj; 42; end))                       # 42
p((class << obj; ancestors; end).is_a?(Array))   # true
p((class << obj; ancestors; end).include?(Object))  # true
p((class << obj; self; end).is_a?(Class))        # true (singleton class)
p((class << obj; ancestors; end).first.to_s.start_with?("#<Class:"))  # true
sc = (class << obj; self; end)
p sc.instance_methods(false).include?(:foo)      # true
