# `Module#instance_methods` and friends — visibility
# filtering. The previous module_introspection commit
# routed all four variants through the same arm because
# rubyrs didn't model visibility-aware filtering. This
# fixture pins the corrected CRuby-faithful semantics:
#
#   instance_methods           public + protected
#   public_instance_methods    public only
#   private_instance_methods   private only
#   protected_instance_methods protected only
#
# Each method's visibility is read from its
# `Method.visibility` cell, which `def` /
# `private` / `protected` / `public` already set during
# class-body compile.

class Foo
  def pub_a; end
  def pub_b; end
  private
  def priv_a; end
  def priv_b; end
  protected
  def prot_a; end
end

puts Foo.instance_methods(false).sort.inspect
puts Foo.public_instance_methods(false).sort.inspect
puts Foo.private_instance_methods(false).sort.inspect
puts Foo.protected_instance_methods(false).sort.inspect

# Inherited walk respects visibility per ancestor.
class Bar < Foo
  def bar_pub; end
  private
  def bar_priv; end
end
puts Bar.public_instance_methods(false).sort.inspect
puts Bar.private_instance_methods(false).sort.inspect
puts Bar.protected_instance_methods(false).sort.inspect

# `instance_methods` on the subclass with inherited=true
# walks both classes; private from either are excluded.
inherited_pub = Bar.instance_methods.sort
expected_pub = [:bar_pub, :pub_a, :pub_b, :prot_a]
expected_pub.each { |m| puts inherited_pub.include?(m) }
expected_excluded = [:bar_priv, :priv_a, :priv_b]
expected_excluded.each { |m| puts inherited_pub.include?(m) }

# `private_instance_methods` with inherited=true picks
# up private methods from both classes.
inherited_priv = Bar.private_instance_methods.sort
puts inherited_priv.include?(:bar_priv)
puts inherited_priv.include?(:priv_a)
puts inherited_priv.include?(:priv_b)
puts inherited_priv.include?(:pub_a)         # false — public, not private
puts inherited_priv.include?(:prot_a)        # false — protected, not private
