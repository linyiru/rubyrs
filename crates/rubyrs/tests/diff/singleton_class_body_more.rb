# Broader `class << self` body subset: (1) explicit-receiver
# statements (`Foo.bar = expr`) run in the surrounding context, and
# (2) visibility modifiers with method-name args (`private :new`) are
# accepted as no-ops (rubyrs doesn't model singleton-method
# visibility). Discovery: P3 Jekyll spike — Liquid's template.rb and
# tag.rb.

class Template
  class << self
    attr_accessor :default_exception_renderer
    # explicit-receiver assignment inside class << self
    Template.default_exception_renderer = lambda { |e| "handled:#{e}" }
  end
  def self.show(x); default_exception_renderer.call(x); end
end
p Template.default_exception_renderer.call("boom")
p Template.show("X")

class Tag
  class << self
    def parse(x); new_via(x); end
    private :new          # no-op (singleton visibility unmodeled)
  end
  def self.new_via(x); "tag:#{x}"; end
end
p Tag.parse("p")

# explicit-receiver method call (not just assignment) inside body
class Reg
  @items = []
  class << self
    attr_reader :items
    Reg.items.push(:a)
    Reg.items.push(:b)
  end
end
p Reg.items
