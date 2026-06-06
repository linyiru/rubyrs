# Runtime (explicit-receiver) attr_reader / attr_writer / attr_accessor
# on a Class — the dispatch-time sibling of the compile-time bareword
# class-body form. Discovery: P3 Jekyll spike — Liquid does
# `singleton_class.send(:attr_accessor, :cache_classes)`.

# explicit Class.attr_* installs INSTANCE accessors + returns names.
class Inst; end
p Inst.attr_accessor(:a)
p Inst.attr_reader(:b)
p Inst.attr_writer(:c)
i = Inst.new
i.a = 10
p i.a
i.c = 7
p i.instance_variable_get(:@c)
p i.instance_variable_set(:@b, 3)
p i.b

# singleton_class.send(:attr_accessor, ...) → CLASS-level accessors.
class Conf
  singleton_class.send(:attr_accessor, :cache_classes)
  self.cache_classes = true
end
p Conf.cache_classes
Conf.cache_classes = 42
p Conf.cache_classes

# String-name args work too.
class Strs; end
p Strs.attr_accessor("x")
s = Strs.new
s.x = "hi"
p s.x

# multiple names at once.
class Multi; end
p Multi.attr_accessor(:p, :q)
