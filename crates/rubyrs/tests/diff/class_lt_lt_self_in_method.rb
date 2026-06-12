# `class << self` inside a METHOD body targets the runtime self's
# eigenclass (minitest's i_suck_and_my_tests_are_order_dependent!).
class Base
  def self.test_order; :random; end
end
c = Class.new(Base)
def c.apply!
  class << self
    undef_method :test_order if method_defined? :test_order
    define_method :test_order do :alpha end
  end
end
c.apply!
p c.test_order
d = Class.new(Base)
def d.apply_def!
  class << self
    def test_order; :alpha_def; end
  end
end
d.apply_def!
p d.test_order
# instance receiver too
o = Object.new
def o.install!
  class << self
    define_method(:dyn) { :odyn }
  end
end
o.install!
p o.dyn
