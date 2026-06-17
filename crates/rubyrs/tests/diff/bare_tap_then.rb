# Bare `tap` / `yield_self` (implicit self) inside an instance method
# must dispatch on self — like the explicit `self.tap`. mail's
# CommonField#parse does `tap(&:element)`. (`then` is a keyword, so a
# bare `then { }` is a parse error — only the `.then` form exists.)
class Widget
  def initialize; @log = []; end
  def element; @log << :el; "EL"; end
  def parse_symblock; tap(&:element); end       # &:sym block-arg form
  def parse_block; tap { |w| w.element }; end    # brace-block form
  def via_yieldself; yield_self { |w| w.class.name }; end
  def via_dot_then; self.then { |w| w.element.length }; end
  def log; @log; end
end
w = Widget.new
p w.parse_symblock.equal?(w)   # true (tap returns self)
p w.parse_block.equal?(w)      # true
p w.log                        # [:el, :el]
p w.via_yieldself              # "Widget"
p w.via_dot_then               # 2

# bare tap returning self for chaining
class Builder
  def initialize; @parts = []; end
  def add(x); @parts << x; self; end
  def build; tap { |b| b.add(:done) }; end
  def parts; @parts; end
end
b = Builder.new.add(:a).build
p b.parts                      # [:a, :done]
