# A Hash subclass's non-mutating builders return instances of the
# subclass (CRuby), so an IndifferentHash-style override survives a
# merge — the wall that lost Sinatra's route params.
class IH < Hash
  def [](k); super(k.to_s); end
end
h = IH.new
h["id"] = "42"
p h.class.name
p h[:id]
m = h.merge({ "x" => "1" })
p [m.class.name, m[:id]]
mb = h.merge({ "id" => "99" }) { |k, o, n| o }   # block-form keeps old
p [mb.class.name, mb[:id]]
# merge! mutates self → stays the subclass
h.merge!({ "y" => "2" })
p [h.class.name, h[:y]]
# plain Hash merge is unaffected
p({ "a" => 1 }.merge({ "b" => 2 }).class.name)
p({ "a" => 1 }.merge({ "a" => 9 }) { |k, o, n| o + n })
