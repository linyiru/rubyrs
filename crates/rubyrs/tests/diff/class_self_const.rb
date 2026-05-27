## `class << self` body now accepts `ConstantWriteNode`. Closes
## TRY_RUNS pass-9 layer #11 — the sinatra/base.rb:1292 case
## opens a singleton class body with
## `CALLERS_TO_IGNORE = [...].freeze` before its `attr_reader`
## and `def` blocks. Before this PR the translator compiled the
## unsupported form into a runtime `raise NotImplementedError`
## that fired at file-load time; the constant assignment is now
## routed through the regular toplevel `Expr::ConstWrite` path.
##
## rubyrs's spike-scope constants model collapses to a single
## name-keyed `Vm.constants` table — so a bare `BAR` read from
## ANY context resolves through that table. This fixture pins
## that read shape, the `.freeze` chain on the RHS, and
## interleaving with the other already-supported `class << self`
## body forms (`attr_reader` / `def`).

class WithConst
  class << self
    BAR = 42
    BAZ = [1, 2, 3].freeze
    QUX = "hello".freeze

    attr_reader :sentinel

    def get_bar; BAR; end
    def get_baz; BAZ; end
    def get_qux; QUX; end
  end
end

## Constant assignment landed; readable from the singleton methods.
puts "bar=#{WithConst.get_bar}"
puts "baz=#{WithConst.get_baz.inspect}"
puts "qux=#{WithConst.get_qux.inspect}"

## rubyrs flattens constant scope in the spike — bare BAR / BAZ
## resolves the same const from anywhere. CRuby places these on
## the singleton class; `Class#singleton_class::CONST` reaches
## them but `Class::CONST` (outer-class scope) and toplevel bare
## reads do NOT. This is a KNOWN GAP at the constants model
## level — out of scope for this PR.
##
## What this fixture CAN pin (both interpreters agree):
##   - bare reads via singleton methods defined in the same
##     `class << self` body (`def get_bar; BAR; end` — verified
##     above).
##   - explicit `Class#singleton_class::CONST` access (the one
##     CRuby form the spike-scope model also happens to match).
##
## What this fixture CANNOT pin: the divergent paths
## (`WithConst::BAR` returns 42 in rubyrs / NameError in CRuby;
## toplevel `BAR` likewise). The diff harness requires byte-
## identical stdout — any assertion that hits the divergent
## branches would always fail one interpreter regardless of
## structure. A future scope-tightening change that lands true
## per-class const tables would be regression-checked by the
## bare-via-singleton-method path going stale (it currently
## depends on the flat-table behavior), surfacing the same
## signal at that point.
puts "via-singleton-class=#{WithConst.singleton_class::BAR}"

## `attr_reader` from the same body still works (alias / def /
## attr_* are pre-existing supported forms — pin to confirm the
## CWN arm didn't break their precedence).
WithConst.instance_variable_set(:@sentinel, "set-via-ivar")
puts "sentinel=#{WithConst.sentinel.inspect}"
