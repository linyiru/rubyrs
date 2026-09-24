# Module#public_instance_method — `instance_method` restricted to
# public methods; private/protected/undefined names raise NameError.
# ActiveSupport's `delegate` probes signatures through it.
class PimHost
  def pub(a, b = 1, *rest, k:, &blk); end
  private def priv; end
  protected def prot; end
end

class PimChild < PimHost; end

um = PimHost.public_instance_method(:pub)
p um.class
p um.name
p um.parameters
p PimHost.public_instance_method("pub").name
p PimChild.public_instance_method(:pub).parameters
p PimHost.public_instance_method(:to_s).name

[:priv, :prot, :missing].each do |m|
  begin
    PimHost.public_instance_method(m)
    p [m, :no_error]
  rescue NameError => e
    p [m, e.class]
  end
end
