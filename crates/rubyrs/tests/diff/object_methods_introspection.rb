# Object#methods / #public_methods / #private_methods /
# #protected_methods / #singleton_methods — receiver-side
# method introspection.
#
# The pre-existing `methods` arm walked the class chain without
# filtering by visibility (so `c.methods.include?(:c_priv)`
# returned true, diverging from CRuby). This commit refactors
# the walk to collect (SymId, Visibility) pairs and adds the
# four visibility-filtered variants plus `singleton_methods`.
#
# Class introspection counterparts (`Module#instance_methods` /
# `public_instance_methods` / `private_instance_methods` /
# `protected_instance_methods`) already existed; this PR
# brings the receiver-side surface up to parity.
#
# Caveat: rubyrs's parser/compiler recognizes block-style
# `private` (header line on its own) but not the inline form
# `private def foo`; this fixture uses block-style to stay
# orthogonal to the parser-level gap.

module M
  def m_pub; end
end

class C
  include M
  def c_pub; end
  private
  def c_priv; end
  protected
  def c_pro; end
end

c = C.new
def c.sing; end

# singleton_methods — only methods installed on the eigenclass
puts c.singleton_methods.inspect              # [:sing]
puts C.new.singleton_methods.inspect          # [] (no eigenclass installed)

# Class receiver — its own class-method table
class K; class << self; def k_cls; end; end; end
puts K.singleton_methods.include?(:k_cls)

# public_methods filters visibility
puts c.public_methods.include?(:c_pub)        # true — defined public
puts c.public_methods.include?(:c_priv)       # false — private excluded
puts c.public_methods.include?(:m_pub)        # true — inherited from module
puts c.public_methods.include?(:sing)         # true — singleton method (public by default)

# private_methods only includes private-marked methods
puts c.private_methods.include?(:c_priv)
puts c.private_methods.include?(:c_pub)       # false

# protected_methods only includes protected
puts c.protected_methods.include?(:c_pro)
puts c.protected_methods.include?(:c_pub)     # false
puts c.protected_methods.include?(:c_priv)    # false

# methods = public + protected (CRuby default)
puts c.methods.include?(:c_pub)
puts c.methods.include?(:c_pro)
puts c.methods.include?(:c_priv)              # false — private excluded
puts c.methods.include?(:sing)                # true — singleton

# Return type is Array<Symbol> in all cases
puts c.public_methods.class.name
puts c.singleton_methods.class.name
puts c.methods.is_a?(Array)
puts c.methods.first.is_a?(Symbol) || c.methods.empty?

# Receivers without a class to walk get an empty Array.
# (rubyrs subset: doesn't synthesize Kernel-level entries for
# every primitive; CRuby would list :+, :-, etc. for Integer.)
puts 42.singleton_methods.inspect
puts nil.singleton_methods.inspect

# respond_to? must agree with dispatch (universal whitelist)
puts 42.respond_to?(:singleton_methods)
puts 42.respond_to?(:public_methods)
puts Object.new.respond_to?(:private_methods)
