# `include M` must invoke M's overridden `append_features` (not just the
# `included` hook), and that override's `super` must perform the real ancestor
# insertion. This is exactly how ActiveSupport::Concern (the foundation of
# Rails) wires up ClassMethods + the included block.
module Traceable
  def self.append_features(base)
    base.instance_variable_set(:@traced, true)
    super                       # the real insert
    base.extend(ClassMethods)
  end
  module ClassMethods
    def traced? = instance_variable_get(:@traced)
  end
  def trace = "traced:#{self.class}"
end

class Widget
  include Traceable
end
p Widget.ancestors.include?(Traceable)   # true (super inserted it)
p Widget.new.trace                        # "traced:Widget" (instance method)
p Widget.traced?                          # true (ClassMethods extended)
p Widget.instance_variable_get(:@traced)  # true (append_features body ran)

# prepend_features twin
module Loud
  def self.prepend_features(base); super; end
  def shout = "LOUD"
end
class Box; prepend Loud; end
p Box.ancestors.first == Loud             # true (prepended at front)
p Box.new.shout                           # "LOUD"
