# The BetterError trio: Kernel#binding returns an opaque Binding
# (un-marshalable); raise dispatches a USER set_backtrace override
# (CRuby funcalls it — minitest's fixture stamps a binding ivar
# there to poison marshalability and trigger the neuter chain);
# the super chain of a class-method hook reaches a hook aliased
# onto Class's instance table.
p binding.class
begin
  Marshal.dump(binding)
rescue TypeError => e
  puts "dump: #{e.message}"
end
class BetterErr < RuntimeError
  def set_backtrace(bt)
    super
    @bad_ivar = binding
  end
end
begin
  raise BetterErr, "boom"
rescue => e
  p e.class
  p e.message
  p e.backtrace.first.include?("binding_set_backtrace")
  p e.instance_variables
  begin
    Marshal.dump(e)
  rescue TypeError
    puts "neuter-trigger: TypeError"
  end
end
# plain raise unaffected (no override → direct backtrace write)
begin
  raise "plain"
rescue => e
  p e.backtrace.first.include?("binding_set_backtrace")
end
# super from a registering class-method hook reaches Class-level alias
class Reg
  def self.inherited(k)
    super
  end
end
Class.class_eval do
  def inherited_hack(_k); throw :hooked; end
  alias inherited_orig inherited
  alias inherited inherited_hack
end
r = catch(:hooked) do
  Class.new(Reg)
  :not_thrown
end
p r
Class.class_eval do
  alias inherited inherited_orig
  undef_method :inherited_hack
  undef_method :inherited_orig
end
p Class.new(Reg).is_a?(Class)
