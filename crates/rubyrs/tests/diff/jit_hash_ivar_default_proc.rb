# Regression (value-JIT correctness): a method whose body is `@h[k]` (the value-JIT
# `HashAttr` pattern) must honor the Hash's `default` / `default_proc` on a MISS.
# `jit_hash_get_value` scanned only the stored pairs and wrote `nil` on a miss,
# ignoring the default — so a defaulted Hash returned nil under jit-native. RuboCop's
# `Config#for_cop` (`@for_cop[cop]`, a default_proc Hash that computes-and-caches the
# cop config) hit this: `cop_config` became nil → `Naming/InclusiveLanguage` crashed
# and the whole run aborted. Parity must hold interpreter == JIT == CRuby.

class Store
  def initialize(&blk); @h = Hash.new(&blk); end
  def get(k); @h[k]; end            # <- the `@ivar[arg]` value-JIT pattern
end

class StoreV
  def initialize(default); @h = Hash.new(default); end
  def get(k); @h[k]; end
end

# default_proc that computes AND caches (the for_cop shape)
s = Store.new { |h, k| h[k] = "computed:#{k}" }
100_000.times { s.get(:warm) }      # make get() hot -> value-JIT compiles it
p s.get(:warm)                      # present (cached)  -> "computed:warm"
p s.get(:fresh)                     # MISS -> default_proc runs -> "computed:fresh"
p s.get(:fresh)                     # now cached        -> "computed:fresh"

# plain default VALUE (no proc)
v = StoreV.new(99)
100_000.times { v.get(1) }          # 1 is absent -> default 99 each time
p v.get(1)                          # 99
p v.get(2)                          # 99

# no default -> nil on a miss (must stay nil, not garbage)
n = StoreV.new(nil)
100_000.times { n.get(7) }
p n.get(7)                          # nil

# integer keys + default_proc (present-after-first vs fresh)
c = Store.new { |h, k| h[k] = k * 10 }
100_000.times { c.get(5) }
p c.get(5)                          # 50 (cached)
p c.get(9)                          # 90 (fresh via proc)
