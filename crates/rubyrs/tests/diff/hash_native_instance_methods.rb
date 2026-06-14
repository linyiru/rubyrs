# Hash's native methods ([], merge, dig, ...) are dispatched from VM arms,
# not stored in the Hash class's method table, so reflection
# (`instance_methods` / `public_instance_methods`) used to miss them.
# CRuby lists them on Hash AND every subclass; this asserts the same so a
# Hash subclass's override set is a SUBSET of Hash.public_instance_methods.
# (rack's Rack::Headers#test_public_interface relies on this cancellation.)
#
# NB: the *absolute* method count differs from CRuby (inherited Enumerable
# / Comparable / Kernel methods aren't enumerated yet), so this fixture
# only probes the subset relationship — which is what the rack test needs.

p Hash.public_instance_methods.include?(:[])       # true
p Hash.public_instance_methods.include?(:[]=)      # true
p Hash.public_instance_methods.include?(:merge)    # true
p Hash.public_instance_methods.include?(:dig)      # true
p Hash.public_instance_methods.include?(:fetch)    # true
p Hash.public_instance_methods.include?(:transform_keys!)  # true

# also surfaced via the public-or-protected `instance_methods`
p Hash.instance_methods.include?(:store)           # true

# a subclass that only OVERRIDES hash methods adds nothing new
class PureOverride < Hash
  def [](k) = super
  def merge(*a) = super
  def dig(*a) = super
end
p (PureOverride.public_instance_methods - Hash.public_instance_methods).sort   # []

# a subclass with a genuinely new method surfaces only that one
class WithExtra < Hash
  def [](k) = super
  def brand_new_method; end
end
p (WithExtra.public_instance_methods - Hash.public_instance_methods).sort      # [:brand_new_method]

# the subclass inherits every native Hash method (reverse direction empty)
p (Hash.public_instance_methods - WithExtra.public_instance_methods).sort      # []

# private/protected variants do NOT pick up the (public) native methods
p Hash.private_instance_methods.include?(:[])      # false
