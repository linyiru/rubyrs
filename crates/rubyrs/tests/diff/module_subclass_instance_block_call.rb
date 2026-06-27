# A method-with-block call on an instance of a `< Module` class resolves
# instance methods (incl. from included modules) from its class — the block
# form must match the no-block path (AR's GeneratedAttributeMethods, a Module
# subclass instance that `include`s a synchronize-providing module, calls
# `generated_attribute_methods.synchronize do … end`).
module Runner
  def run_each
    yield 1
    yield 2
    "done"
  end
end
class Tagged < Module
  include Runner
  def direct_block
    yield 10
  end
end
g = Tagged.new
acc = []
p g.run_each { |x| acc << x }   # included module method, with block
p acc
g.direct_block { |x| acc << x } # own method, with block
p acc
