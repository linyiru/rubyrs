# Object.instance_method(:X) resolves universal Kernel/Object methods to
# UnboundMethods (rspec-mocks captures instance_variable_get this way);
# Class/Module.instance_method(:new|:name|...) returns a (deferred)
# UnboundMethod too.
%i[instance_variable_get instance_variable_set instance_variable_defined?
   instance_variables freeze dup clone tap class frozen? object_id].each do |m|
  p Object.instance_method(m).class
end
# bind+call the captured handle (rspec idiom: read an ivar past overrides)
o = Object.new
o.instance_variable_set(:@x, 7)
p Object.instance_method(:instance_variable_get).bind(o).call(:@x)   # 7
p Object.instance_method(:instance_variable_get).arity                # 1
# Class#new as an UnboundMethod, bound + called to build an instance
um = Class.instance_method(:new)
p um.class                                                            # UnboundMethod
k = Class.new { def initialize(v); @v = v; end; attr_reader :v }
p um.bind(k).call(99).v                                               # 99
p Module.instance_method(:name).class                                # UnboundMethod
