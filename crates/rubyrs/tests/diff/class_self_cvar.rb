## `class << self` body now accepts `ClassVariableWriteNode`.
## Closes TRY_RUNS pass-9.5 layer #12 — the sinatra/base.rb
## case where a `class << self` body initializes a class
## variable (`@@mutex = Mutex.new`) and singleton methods in
## the same body reach it later.
##
## CRuby places class variables on the enclosing class
## hierarchy regardless of whether the write happens inside
## `class << self` — cvars are hierarchy-keyed in CRuby, not
## singleton-class-scoped. rubyrs's Tier-1 cvar model is
## per-class with no hierarchy walk (pre-existing divergence
## from CRuby — see Op::LoadCvar/StoreCvar). What this PR's
## arm fixes is strictly the PLACEMENT side: the write
## syntactically appearing inside `class << self` lands in
## the same table it would if syntactically at class-body top
## level. The full CRuby cvar semantic (hierarchy lookup) is
## out of scope and the subclass-reach KNOWN GAP below
## captures the remaining divergence. Unlike the layer-#11
## constant case (PR #209) — where the spike-scope flat
## constants table introduces a NEW cross-scope divergence on
## `Class::CONST` reads — this PR doesn't add a new divergence
## point; it preserves the existing one.

class WithCvar
  class << self
    @@counter = 0
    @@names   = []

    def bump
      @@counter += 1
    end

    def add(name)
      @@names << name
    end

    def stats
      [@@counter, @@names + []]
    end
  end
end

## Cvar assignment landed; readable + mutable from singleton
## methods defined in the same body.
WithCvar.bump
WithCvar.bump
WithCvar.add("a")
WithCvar.add("b")
puts "stats=#{WithCvar.stats.inspect}"

## KNOWN GAP: rubyrs's Tier-1 class variable model is per-class
## (no hierarchy walk on read or write — see `Op::LoadCvar` /
## `StoreCvar`). A subclass that reads a cvar set by its parent
## sees nil instead of walking up the chain — CRuby's cvars
## are hierarchy-keyed, so the same code works there. Pre-
## dates this PR (cvar storage / lookup, not the AST whitelist).
## Not exercised here so the fixture stays byte-aligned with
## CRuby; flagged for a separate follow-up.

## Interleaving with the layer-#11 const form (PR #209) plus
## attr_*/def — pin to confirm the new arm didn't displace the
## earlier arms' precedence in the body translator.
class MixedBody
  class << self
    LIMIT = 3
    @@hits = 0

    attr_reader :tag

    def tick
      @@hits += 1 if @@hits < LIMIT
      @@hits
    end
  end
end

3.times { MixedBody.tick }
MixedBody.tick   # capped at LIMIT
MixedBody.instance_variable_set(:@tag, "mixed")
puts "mixed-hits=#{MixedBody.tick}"
puts "mixed-tag=#{MixedBody.tag.inspect}"
puts "mixed-limit=#{MixedBody.singleton_class::LIMIT}"
