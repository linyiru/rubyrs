# Reopening a `Struct.new`-created class that was assigned to a constant
# INSIDE a module must reopen the SAME class — preserving its struct
# members, its `extend`ed singleton methods, and any instance methods —
# not mint a fresh empty class. The anon class is named/keyed by its full
# scoped path (`Ns::Thing`), so the reopen (which keys by the qualified
# name) finds it. Surfaced by faraday: `Request = Struct.new(…) { extend
# MiddlewareRegistry }` then `module Faraday; class Request; …` reopen.
module Ns
  module Reg
    def registry_marker; :registered; end
  end
  Thing = Struct.new(:a, :b) do
    extend Reg
    def combined; "#{a}+#{b}"; end
  end
end

# Qualified name (CRuby: "Ns::Thing", not bare "Thing").
p Ns::Thing.name
p Ns::Thing.registry_marker

# Reopen via the nested form (authorization.rb shape).
module Ns
  class Thing
    class Sub
      LABEL = "sub-label"
    end
    def extra; "extra-method"; end
  end
end

# extend'd class method, struct members, original + new instance methods,
# and the nested constant all survive the reopen.
p Ns::Thing.registry_marker
t = Ns::Thing.new(10, 20)
p t.a
p t.b
p t.combined
p t.extra
p Ns::Thing::Sub::LABEL
