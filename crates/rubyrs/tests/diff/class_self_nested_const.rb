# A nested module/class/CONST defined directly inside a `class << self`
# body is scoped to the eigenclass (CRuby). rubyrs additionally registers
# it on the eigenclass's own const table so explicit access via
# `singleton_class.const_get/const_defined?(:X, false)` works, while bare
# lexical reads from eigenclass-body methods keep resolving.
class Loader
  class << self
    module Synchronized
      def cattr; 42; end
    end
    CONFIG = {a: 1}
    def read_config; CONFIG; end          # bare lexical read from eigenclass body
    def read_sync; Synchronized; end      # bare lexical read of nested module
  end
end
sc = Loader.singleton_class
p sc.const_defined?(:Synchronized, false)
p sc.const_get(:Synchronized).instance_method(:cattr).is_a?(UnboundMethod)
p sc.const_defined?(:CONFIG, false)
p sc.const_get(:CONFIG)
p Loader.read_config
p(Loader.read_sync.name =~ /Synchronized/ ? :ok : :no)
