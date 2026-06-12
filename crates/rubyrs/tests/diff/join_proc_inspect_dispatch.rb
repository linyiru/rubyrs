# Array#join dispatches user to_s on Object/Class elements (minitest
# Spec's describe-name builder joins [parent_class, desc]); Proc
# inspect/to_s render the CRuby file:line form.
class NamedThing
  def to_s; "custom"; end
end
p [NamedThing.new, "x"].join("::")
c = Class.new { def self.to_s; "KlassName"; end }
p [c, :leaf].join("::")
pr = proc { 1 }
s = pr.inspect.gsub(/0x[0-9a-f]+/, "0xX")
puts s.sub(/[^ ]+:\d+/, "FILE:LINE")
p pr.inspect == pr.to_s
