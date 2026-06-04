# Minimal `delegate` stdlib shim. Real CRuby DelegateClass
# reflects on the wrapped class's `instance_methods(false)`
# list at class-creation time and defines each as an explicit
# forwarder. rubyrs's primitive-class `instance_methods(false)`
# returns empty (Hash's methods live in the dispatch tables,
# not the user-method table), so reflection-driven generation
# can't iterate the list. This shim uses `method_missing`
# forwarding instead — slower in theory but functionally
# equivalent for the sinatra-flash FlashHash use case.

def DelegateClass(klass)
  Class.new do
    def initialize(obj)
      @_delegate = obj
    end
    define_method(:method_missing) do |name, *args, &block|
      if @_delegate.respond_to?(name)
        @_delegate.send(name, *args, &block)
      else
        super(name, *args, &block)
      end
    end
    define_method(:respond_to_missing?) do |name, include_private = false|
      @_delegate.respond_to?(name, include_private)
    end
    define_method(:__getobj__) { @_delegate }
  end
end
