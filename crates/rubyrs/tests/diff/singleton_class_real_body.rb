# Real eigenclass-body execution (`class << <expr>` run with
# self = the metaclass), as opposed to the def/attr/alias desugar.
# Models zeitwerk's Zeitwerk::ExplicitNamespace: `include` of a
# module into the metaclass, `extend` of a helper that wraps
# `def`, `attr_reader` + `private`, and the `internal def` /
# `private def` keyword-wrapped-def idioms.

module RealModName
  def real_mod_name(mod) = "named:#{mod}"
end

module Internal
  # `internal def foo` → foo is a class method; make it private and
  # expose a public `foo_internal` alias (independent visibility).
  def internal(method_name)
    private method_name
    alias_method "#{method_name}_internal", method_name
    public "#{method_name}_internal"
    method_name
  end
end

module ExplicitNamespace
  class << self
    include RealModName
    extend Internal

    attr_reader :cpaths
    private :cpaths

    internal def register(cpath)
      "registered:#{cpath}"
    end

    private def helper
      "helper-result"
    end

    def public_api
      "via:#{register_internal('p')}+#{helper}"
    end
  end
end

# include RealModName → real_mod_name is a class method, and `self`
# inside the body was the metaclass.
p ExplicitNamespace.real_mod_name(:Foo)

# attr_reader :cpaths + private :cpaths → private singleton reader.
p ExplicitNamespace.respond_to?(:cpaths)
p ExplicitNamespace.respond_to?(:cpaths, true)

# internal def register → register private, register_internal public.
p ExplicitNamespace.respond_to?(:register)
p ExplicitNamespace.respond_to?(:register, true)
p ExplicitNamespace.register_internal("a/b")

# private def helper → helper is a private class method.
p ExplicitNamespace.respond_to?(:helper)
p ExplicitNamespace.respond_to?(:helper, true)

# public method reaches the private ones via internal self-dispatch.
p ExplicitNamespace.public_api

# Plain class methods on the real class, not instance methods.
p ExplicitNamespace.instance_methods(false)
p ExplicitNamespace.methods(false).sort
