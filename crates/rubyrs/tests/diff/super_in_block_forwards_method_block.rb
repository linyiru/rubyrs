# `super` inside a block forwards the ENCLOSING METHOD's block, so the
# superclass method's `yield` reaches it. concurrent-ruby's
# `compute_if_absent(key) { @lock.synchronize { super } }` over a yielding
# base is the forcing case.
class Base
  def cia(key)
    yield key
  end
end
class Sub < Base
  def wrap
    yield
  end
  def cia(key)
    wrap { super }
  end
end
p Sub.new.cia(5) { |k| k * 10 }
# nested blocks: super two blocks deep still finds the method block
class Sub2 < Base
  def two; yield; end
  def cia(key)
    two { two { super } }
  end
end
p Sub2.new.cia(3) { |k| k + 100 }
