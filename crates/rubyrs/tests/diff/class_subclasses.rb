# Ruby 3.1+ `Class#subclasses` — immediate (non-transitive) subclasses.
# Surfaced by bridgetown-foundation's `Class#descendants` (recurses over
# `subclasses`) instantiating converter/generator classes in Site.new.
# Order is unspecified in CRuby, so sort for a deterministic diff.
class Base; end
class A < Base; end
class B < Base; end
class C < A; end
module Mixin; end
class A; include Mixin; end   # include must NOT add a subclass entry
p Base.subclasses.map(&:name).sort
p A.subclasses.map(&:name).sort
p C.subclasses
p Mixin.respond_to?(:subclasses) ? Mixin.subclasses : "n/a"
