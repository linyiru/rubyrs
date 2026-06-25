# Implicit-self (no_recv) method dispatch on an Object self — the ADR
# 0031 increment-1 fast path. Must resolve identically to the slow path:
# private/protected callable via implicit self, singletons, inheritance,
# blocks, polymorphic self, method_missing.
class Base
  def pub(x); "pub#{x}"; end
  private def priv(x); "priv#{x}"; end   # callable via implicit self
  def use_priv; priv(1); end             # bare call to a private method
  def use_pub;  pub(2);  end
  def with_block; [1,2].map { each_helper(_1) }; end  # implicit call inside a block
  def each_helper(n); n * 10; end
  def via_mm; ghost(9); end              # method_missing path (no such method)
  def method_missing(n, *a); "mm:#{n}:#{a.inspect}"; end
  def respond_to_missing?(n, p=false); n == :ghost || super; end
end
class Sub < Base
  def use_inherited; pub(3); end         # inherited method via implicit self
  def pub(x); "sub#{x}"; end             # override (polymorphic)
end
b = Base.new
p b.use_priv
p b.use_pub
p b.with_block
p b.via_mm
p Sub.new.use_inherited
p Sub.new.use_pub                        # Sub#pub override via implicit self -> "sub2"
# singleton method on a specific instance, called via implicit self
o = Base.new
def o.special; helper_s; end
def o.helper_s; "singleton"; end
p o.special
