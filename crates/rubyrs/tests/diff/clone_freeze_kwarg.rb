# Object#clone(freeze: true|false|nil) — CRuby's freeze override. Only
# clone accepts it; dup rejects any arg. nil/absent keeps the source's
# frozen state, true/false force it.
def m(&b); b.call; rescue => e; "#{e.class}: #{e.message}"; end
# Array
p [1, 2].freeze.clone.frozen?
p [1, 2].freeze.clone(freeze: false).frozen?
p [1, 2].clone(freeze: true).frozen?
p [1, 2].clone(freeze: nil).frozen?
p [1, 2].freeze.clone(freeze: nil).frozen?
# String / Hash
p "x".freeze.clone(freeze: false).frozen?
p "x".clone(freeze: true).frozen?
p({ a: 1 }.freeze.clone(freeze: false).frozen?)
p({ a: 1 }.clone(freeze: true).frozen?)
# Object + singleton preserved
o = Object.new.freeze
p o.clone.frozen?
p o.clone(freeze: false).frozen?
s = Object.new
def s.foo; 42; end
p s.clone(freeze: false).foo
# dup never takes freeze:
p m { [1, 2].dup(freeze: false) }
# unknown keyword
p m { [1, 2].clone(other: 1) }
# clone copies contents
p [1, 2, 3].freeze.clone(freeze: false).map { |x| x * 2 }
