# A constant assigned inside a `class << <const>` body lives on the
# eigenclass and is referenced BARE by singleton methods in the same
# body — which only resolves when those methods carry the eigenclass
# cref. Routed to the real eigenclass-body path. Surfaced by diff-lcs
# (`class << Diff::LCS; PATCH_MAP = {…}; def … PATCH_MAP[dir] … end`).
module Diff
  class LCS; end
end
class << Diff::LCS
  PATCH_MAP = { patch: :p, unpatch: :u }.freeze
  def lookup(dir); PATCH_MAP[dir]; end
  def both; [PATCH_MAP[:patch], PATCH_MAP[:unpatch]]; end
  def hidden; :secret; end
  private :hidden
end
p Diff::LCS.lookup(:patch)        # :p
p Diff::LCS.both                  # [:p, :u]
begin; Diff::LCS.hidden; rescue NoMethodError; puts "hidden private"; end
p Diff::LCS.singleton_class::PATCH_MAP[:unpatch]   # :u

# def + alias + const together
class Box; end
class << Box
  KIND = :box
  def kind; KIND; end
  alias_method :type, :kind
end
p Box.kind                        # :box
p Box.type                        # :box
