# respond_to? surface parity bits minitest probes.
p "a".respond_to?(:=~)
p "a".respond_to?(:exit)
p "a".respond_to?(:exit, true)
p 1.respond_to?(:puts, true)
p nil.respond_to?(:warn, true)
class RFoo
  private
  def secret; end
end
p RFoo.new.respond_to?(:secret)
p RFoo.new.respond_to?(:secret, true)
