# Forwardable's `def_delegator` / `instance_delegate` with a
# DOTTED accessor (`'self.class'`, `'a.b'`) — the accessor string
# is a RECEIVER EXPRESSION, not a single method name. CRuby
# splices it verbatim into the generated body; the rubyrs shim
# now walks the dotted chain via __send__ (a leading `self`
# segment is the delegating object itself).
#
# Discovery: P3 Sinatra spike — mustermann/ast/pattern.rb:23
# `instance_delegate [...] => 'self.class'` previously raised
# `NoMethodError: undefined method 'self.class'`.

require 'forwardable'

# Shape 1: instance_delegate to 'self.class' — delegates to a
# class method, resolved freshly so subclasses see their own.
class Box
  class << self
    def kind; "kind-of-#{name}"; end
    def build(x); "built-#{x}-by-#{name}"; end
  end
  extend Forwardable
  instance_delegate [:kind, :build] => 'self.class'
end
class Sub < Box; end

puts "box_kind=#{Box.new.kind}"
puts "sub_kind=#{Sub.new.kind}"
puts "box_build=#{Box.new.build(7)}"
puts "sub_build=#{Sub.new.build(9)}"

# Shape 2: def_delegator with the dotted accessor directly.
class Widget
  class << self
    def label; "W"; end
  end
  extend Forwardable
  def_delegator 'self.class', :label, :my_label
end
puts "widget=#{Widget.new.my_label}"

# Shape 3: a plain (non-dotted) reader accessor still works.
class Holder
  extend Forwardable
  def initialize(v); @v = v; end
  def reader; @v; end
  def_delegator :reader, :upcase, :shout
end
puts "holder=#{Holder.new("hi").shout}"

# Shape 4: an ivar accessor still works.
class IvarBox
  extend Forwardable
  def initialize; @inner = [1, 2, 3]; end
  def_delegator :@inner, :size, :count
end
puts "ivar=#{IvarBox.new.count}"
