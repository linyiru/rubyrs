# String per-instance eigenclass (Vm::str_singletons side-table):
# `def s.foo`, `s.singleton_class`, stub-style define/alias/undef
# through the eigenclass, and `super` from an eigenclass method
# reaching the String primitive. minitest consumers:
# test_stub_yield_self ((+"foo").stub :to_s, "bar") and
# TestMinitestAssertionHelpers#test_diff_equal (def s.==).

s = +"foo"
def s.shout
  upcase + "!"
end
p s.shout
p s.respond_to?(:shout) if s.respond_to?(:shout) # both sides true => prints

# Another string is untouched.
t = +"foo"
p t.respond_to?(:shout)

# singleton_class returns a Class whose methods land on s only.
sc = s.singleton_class
p sc.class
sc.send(:define_method, :riddle) { length * 10 }
p s.riddle
p t.respond_to?(:riddle)

# super from an eigenclass method hits the String primitive.
def s.upcase
  "[" + super + "]"
end
p s.upcase
p t.upcase

# stub-style save/define/restore through the eigenclass: alias the
# primitive away, override, call, then restore + undef the save.
sc.send(:alias_method, :__save_to_s, :to_s)
sc.send(:define_method, :to_s) { "stubbed" }
p s.to_s
p "#{s}"
sc.send(:undef_method, :to_s)
sc.send(:alias_method, :to_s, :__save_to_s)
sc.send(:undef_method, :__save_to_s)
p s.to_s

# Hash-key / equality semantics unchanged by the eigenclass.
p s == "foo"
p({ s => 1 }["foo"])
