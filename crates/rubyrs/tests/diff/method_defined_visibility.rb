# public/private/protected_method_defined? triplet — minitest's
# Spec DSL `it` walks children.reject { |c| c.public_method_defined? name }
# (nested describes only; its absence silently dropped 35 of 76 specs).
class K
  def pub; end
  private def priv; end
  protected def prot; end
end
p K.public_method_defined?(:pub)
p K.public_method_defined?(:priv)
p K.public_method_defined?(:prot)
p K.public_method_defined?(:nope)
p K.private_method_defined?(:priv)
p K.private_method_defined?(:pub)
p K.protected_method_defined?(:prot)
p K.protected_method_defined?(:priv)
p K.public_method_defined?("pub")
p String.public_method_defined?(:length)
class Sub < K; end
p Sub.public_method_defined?(:pub)
p Sub.private_method_defined?(:priv)
