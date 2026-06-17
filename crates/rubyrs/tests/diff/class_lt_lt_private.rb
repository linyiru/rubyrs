# `private :m` / `public :m` (with explicit method-name args) inside a
# `class << X` body set the visibility of X's SINGLETON method `m`.
# Surfaced by diff-lcs's `class << Diff::LCS::Internals; … private
# :diff_traversal`.

# class << <non-self> (constant receiver)
module Lib
  class Internals; end
end
class << Lib::Internals
  def pub(x); x + 1; end
  def priv; :secret; end
  private :priv
end
p Lib::Internals.pub(10)                 # 11
begin; Lib::Internals.priv; rescue NoMethodError => e; puts "priv: NoMethodError"; end

# class << self
class Widget
  class << self
    def shown; :ok; end
    def hidden; :nope; end
    private :hidden
  end
end
p Widget.shown                           # :ok
begin; Widget.hidden; rescue NoMethodError; puts "hidden: NoMethodError"; end
# send still reaches a private singleton method
p Widget.send(:hidden)                   # :nope

# public :m re-exposes
class Gadget
  class << self
    def a; 1; end
    private :a
    public :a
  end
end
p Gadget.a                               # 1
