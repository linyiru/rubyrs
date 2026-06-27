# A class variable resolves through the method's LEXICAL/defining class (CRuby's
# cref), not `self`. When a module's method is reached via `extend`, `self` is
# the host but the cvar lives on the defining module. i18n's
# @@normalized_key_cache (set in I18n::Base, read by Base#normalize_key called as
# I18n.normalize_key through `extend Base`) depends on this.
module Store
  @@items = ["seeded"]
  def all = @@items
  def add(x); @@items << x; @@items; end
end
module Front
  extend Store
end
p Front.all              # ["seeded"] (cvar resolves via Store, not Front)
Front.add("more")
p Front.all              # ["seeded", "more"]

# inheritance: cvar from the defining (parent) class
class Parent
  @@count = 10
  def count = @@count
end
class Child < Parent; end
p Child.new.count        # 10
