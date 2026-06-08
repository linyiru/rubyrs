# Hash#delete(key) { |key| default } — key present: remove + return the
# value (block ignored); key absent: call the block with the key and
# return its result. rouge lexer initializers use
# `opts.delete(:flag) { default }`.
h = {a: 1, b: 2, c: 3}
p h.delete(:a) { :missing }            # 1 (present)
p h.delete(:z) { :missing }            # :missing (absent)
p h.delete(:y) { |k| "no #{k}" }       # "no y"
p h                                     # {b: 2, c: 3}
p h.delete(:nope)                       # nil (no block, absent)
p h.delete(:b)                          # 2 (no block, present)
# block result types
p h.delete(:gone) { [] }               # []
p({}.delete(:x) { 42 })                 # 42
# non-local return from the block
def via_delete(h)
  h.delete(:absent) { return :early }
  :normal
end
p via_delete({})                        # :early
# mutation visible
g = {x: 10}
g.delete(:x) { 0 }
p g                                     # {}
