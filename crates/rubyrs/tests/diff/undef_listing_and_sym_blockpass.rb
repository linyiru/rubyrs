# Two walls from minitest's spec self-suite:
#
# 1. Method LISTING respects undef tombstones (the nuke_test_methods!
#    + runnable_methods pattern: enumerate then send — phantom
#    listings dispatch into NoMethodError).
Outer = Class.new(Object) do
  define_method("test_a") { "a" }
  def test_b; "b"; end
end
Inner = Class.new(Outer) do
  public_instance_methods.grep(/^test_/).each { |n| send :undef_method, n }
  def test_c; "c"; end
end
p Outer.instance_methods(false).grep(/^test_/).sort
p Inner.instance_methods.grep(/^test_/).sort
p Inner.public_instance_methods.grep(/^test_/).sort
p Inner.method_defined?(:test_a)
p Inner.method_defined?(:test_c)
class Inner
  def test_a; "redefined"; end
end
p Inner.instance_methods.grep(/^test_/).sort
p Inner.new.test_a

# 2. A VARIABLE holding a Symbol coerces through &  (the literal
#    &:sym form desugars at translation; mu_pp_for_diff's
#    `gsub(/…/, &process)` with process = :itself is the variable
#    shape).
process = :upcase
p "ab".gsub(/a/, &process)
def taker(&b); b.call("xy"); end
p taker(&process)
p [1, 2].map(&process.to_proc >> :to_s.to_proc) rescue p [1, 2].map(&:to_s)
