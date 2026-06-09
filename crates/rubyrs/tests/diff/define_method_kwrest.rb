# A `define_method` block with a `|**kw|` keyword-rest now binds kwargs
# (previously they leaked into `*rest` / tripped the arity check, and the
# kwrest slot stayed nil instead of defaulting to {}).

c = Class.new

c.define_method(:kw) { |**k| k }
p c.new.kw(x: 1, y: 2)
p c.new.kw                       # empty → {}

c.define_method(:rest_kw) { |*a, **k| [a, k] }
p c.new.rest_kw(1, 2, x: 9)
p c.new.rest_kw(1, 2)            # k → {}
p c.new.rest_kw                  # a → [], k → {}

c.define_method(:pos_kw) { |a, **k| [a, k] }
p c.new.pos_kw(1, x: 2, y: 3)

# regression: rest / block-arg / plain still bind
c.define_method(:rest_only) { |*a| a }
p c.new.rest_only(1, 2, 3)
c.define_method(:blk) { |&b| b.call(10) }
p c.new.blk { |n| n * 2 }
c.define_method(:plain) { |a, b| a + b }
p c.new.plain(3, 4)

# class-body form
class Cfg
  define_method(:set) { |key, **opts| [key, opts] }
end
p Cfg.new.set(:timeout, value: 30, unit: :s)
p Cfg.new.set(:name)

# the kwrest hash is mutable / usable
c.define_method(:merge_in) { |**k| k.merge(extra: true) }
p c.new.merge_in(a: 1)
