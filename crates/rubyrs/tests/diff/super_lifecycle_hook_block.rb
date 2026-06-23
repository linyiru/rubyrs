# A lifecycle hook (`included`/`extended`/`prepended`) defined with a
# block param, calling bare `super` (which forwards the block → the
# Op::ApplySuperBlock path), must reach CRuby's empty default callback
# as a no-op — not raise "super: no superclass method". This is the
# ActiveSupport::Concern shape: `def included(base = nil, &block); ...;
# super; end` extended onto a module, then mixed into a class.

module Concern
  def included(base = nil, &block)
    base.nil? ? "block form" : super
  end
  def prepended(base = nil, &block)
    base.nil? ? "block form" : super
  end
  def extended(base = nil, &block)
    base.nil? ? "block form" : super
  end
end

module M
  extend Concern
end

class C
  include M
end
puts "include ok"

module P
  extend Concern
end
class D
  prepend P
end
puts "prepend ok"

module E
  extend Concern
end
obj = Object.new
obj.extend(E)
puts "extend ok"

# inherited with a block param + super (Class#inherited default no-op).
class Base
  def self.inherited(sub, &block)
    super
  end
end
class Sub < Base
end
puts "inherited ok"
