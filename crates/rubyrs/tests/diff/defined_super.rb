# `defined?(super)` — "super" when the enclosing method has a same-named
# method up the chain, else nil. sorbet's T::Helpers#abstract! gates a
# super call with `if defined?(super)` to play nice with Rails.
class Base
  def greet; "base"; end
end
class Mid < Base
  def greet; defined?(super) ? "mid+#{super}" : "mid"; end
  def solo;  defined?(super) ? "has" : "none"; end   # no super solo
end
p Mid.new.greet      # "mid+base"
p Mid.new.solo       # "none"

# module method extended, no super in chain
module Helper
  def configure!; defined?(super) ? super : :configured; end
end
module Target
  extend Helper
  p configure!       # :configured  (no super configure!)
end

# module method WITH a super (two modules both defining it)
module A
  def hook; "A"; end
end
module B
  def hook; defined?(super) ? "B+#{super}" : "B-only"; end
end
class Host
  extend A
  extend B
end
p Host.hook          # "B+A"

# defined?(super) is a String, usable as a truthy/label
class Lbl < Base
  def greet; defined?(super); end
end
p Lbl.new.greet      # "super"
