# A compact path-name class definition (`class A::B`) inside an
# enclosing module must resolve the head (`A`) against the lexical
# scope and define the last segment in THAT namespace — not mint a
# fresh top-level `A::B`. parser gem's AST builder is defined as
# `module Parser; class Builders::Default; … end; end`.

module Outer
  module Builders
  end
  class Builders::Default
    def tag; :default_builder; end
  end
end

p defined?(Outer::Builders::Default)
p Outer::Builders.constants
p Outer::Builders::Default.new.tag

# Reopening via the same compact path lands on the same class.
module Outer
  class Builders::Default
    def extra; :extra; end
  end
end
d = Outer::Builders::Default.new
p [d.tag, d.extra]

# A bare reference to the path from a sibling class inside the scope.
module Outer
  class Consumer
    def build; Builders::Default.new.tag; end
  end
end
p Outer::Consumer.new.build

# Top-level compact path is NOT prefixed by any enclosing scope.
class TopBase; end
class TopBase::Child
  def who; :top_child; end
end
p TopBase::Child.new.who

# A compact path whose head is top-level, referenced from inside a
# module, must still resolve to the top-level class (no prefixing).
module Wrapper
  class TopBase::Sibling
    def who; :top_sibling; end
  end
end
p TopBase::Sibling.new.who
