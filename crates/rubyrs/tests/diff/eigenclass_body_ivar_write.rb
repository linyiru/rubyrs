# An ivar write in a `class << self` body sets the ivar on the
# singleton class (self there), not on the attached class.
class EcHost
  class << self
    @plain = 1
    @chain_a = @chain_b = nil
    @or_w ||= 3
    @or_w &&= @or_w + 1
    @op_w = 10
    @op_w += 5
    attr_accessor :setting
  end
end

sc = EcHost.singleton_class
p sc.instance_variables.sort
p sc.instance_variable_get(:@plain)
p sc.instance_variable_get(:@or_w)
p sc.instance_variable_get(:@op_w)
p EcHost.instance_variables
EcHost.setting = :ok
p EcHost.setting
p EcHost.instance_variables
