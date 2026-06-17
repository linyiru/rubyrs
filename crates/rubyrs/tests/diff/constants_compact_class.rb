# `Module#constants` must list nested classes/modules defined via the
# COMPACT `class M::Foo` form, not just the `module M; class Foo; end`
# form. Surfaced by regexp_parser, whose version classes are
# `class Regexp::Syntax::V1_8_6 < …` — its `specified_versions`
# (`constants.select { … }`) found none of them.
module M
  class V1_8_6; end
  module Inner; end
  FOO = 1
end
class M::V3_4; end            # compact-form nested class
module M::Deep; end           # compact-form nested module
M::V9_9 = Class.new           # const-assigned class
p M.constants.sort            # [:Deep, :FOO, :Inner, :V1_8_6, :V3_4, :V9_9]

# constants(false) own-only also lists them
p M.constants(false).sort     # same

# select pattern (regexp_parser's specified_versions shape)
versions = M.constants.select { |c| c.to_s =~ /\AV\d/ }
p versions.sort               # [:V1_8_6, :V3_4, :V9_9]

# inherited constants from a compact-form subclass still excluded by (false)
class Base; BC = :bc; end
class M::Child < Base; end
p M::Child.constants.include?(:BC)        # true (inherited)
p M::Child.constants(false).include?(:BC) # false (own-only)
