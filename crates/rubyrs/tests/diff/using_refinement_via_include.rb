# `using M` must activate refinements defined in modules M *includes*,
# not only those defined directly in M. CRuby walks the refinement
# module's ancestors. Surfaced by bridgetown-foundation, whose
# `Bridgetown::Refinements` includes the modules that hold the actual
# `refine ::Hash do … end` blocks.
module Defs
  refine Hash do
    def shout; "HASH=#{self.inspect}"; end
  end
  refine Array do
    def shout; "ARR=#{self.inspect}"; end
  end
end

module Bundle
  include Defs
end

class Consumer
  using Bundle
  def on_hash(h); h.shout; end
  def on_arr(a); a.shout; end
end

puts Consumer.new.on_hash({a: 1})
puts Consumer.new.on_arr([1, 2])
