# A bare (implicit-self) universal method call — inspect / to_s /
# frozen? / nil? / is_a? / hash / dup / … — must dispatch on self even
# when self is a primitive (nil/true/false, Integer, String, Symbol).
# Bare implicit-self IS `self.method`, and these all work via an
# explicit receiver; the bare path previously only routed for
# Object/Class selves and raised NoMethodError otherwise.

class NilClass
  def probe;   inspect; end
  def probe2;  to_s; end
  def isnil;   nil?; end
  def fr;      frozen?; end
end
p nil.probe
p nil.probe2
p nil.isnil
p nil.fr

class TrueClass
  def tprobe; [inspect, nil?, is_a?(Object)]; end
end
p true.tprobe

class FalseClass
  def fprobe; "v=#{inspect} nil?=#{nil?}"; end
end
p false.fprobe

class Integer
  def iprobe; [inspect, frozen?, is_a?(Numeric), nil?]; end
end
p 5.iprobe

class String
  def sprobe; [inspect, nil?, is_a?(Comparable)]; end
end
p "x".sprobe

class Symbol
  def syprobe; [inspect, frozen?]; end
end
p :foo.syprobe

# instance_eval flips self to nil; bare calls inside dispatch on it.
p(nil.instance_eval { [inspect, nil?] })

# Toplevel self (the main object) is unchanged — bare inspect is "main".
p inspect
