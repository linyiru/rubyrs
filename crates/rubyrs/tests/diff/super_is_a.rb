# An is_a? / kind_of? / instance_of? override that calls `super` must
# reach the builtin Kernel type test (no Ruby method sits above the
# override). This is mustermann's Node#is_a? shape:
#   def is_a?(type); type = Map[type] if type.is_a? Symbol; super(type); end
module Tag; end
class Base
  include Tag
end
class Mid < Base
  # normalise a Symbol arg to a class, then super into the real check
  MAP = { node: Base }
  def is_a?(type)
    type = MAP[type] if type.is_a?(Symbol)
    super(type)
  end
  def kind_of?(type)
    super
  end
  def instance_of?(type)
    super
  end
end

m = Mid.new
p m.is_a?(:node)        # MAP[:node] == Base -> true
p m.is_a?(Base)         # true
p m.is_a?(Mid)          # true
p m.is_a?(Tag)          # module include -> true
p m.is_a?(String)       # false
p m.kind_of?(Base)      # true
p m.instance_of?(Mid)   # true
p m.instance_of?(Base)  # false (exact class only)
