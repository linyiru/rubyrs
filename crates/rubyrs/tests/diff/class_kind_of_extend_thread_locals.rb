# Two minitest-spec walls:
# 1. A CLASS OBJECT is kind_of? every module `extend`ed into its
#    metaclass tower, inherited down subclasses (Spec.describe
#    gates on `kind_of?(Minitest::Spec::DSL)`) — bare form too.
# 2. Thread.new bodies start with EMPTY fiber-locals
#    (`Thread.current[:k]`), restored after the deferred run.

module DSL
  def hi
    :hi
  end
end

class Base
  extend DSL
end

class Kid < Base
  def self.probe
    kind_of?(DSL)
  end
end

p Base.kind_of?(DSL)
p Kid.kind_of?(DSL)
p Kid.is_a?(DSL)
p Class.new(Kid).kind_of?(DSL)
p Kid.probe
p Kid.instance_of?(Class)
p Kid.kind_of?(Comparable)
p 5.kind_of?(DSL)

Thread.current[:spec] = :outer
inner = nil
seen_key = nil
t = Thread.new do
  seen_key = Thread.current[:spec]
  Thread.current[:spec] = :inner
  inner = Thread.current[:spec]
  :done
end
p t.value
p seen_key
p inner
p Thread.current[:spec]
