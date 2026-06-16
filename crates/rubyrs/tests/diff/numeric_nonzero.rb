# `Numeric#nonzero?` — self when non-zero, nil when zero. Surfaced by
# signalize.rb's `_dispose` (`@_targets.nonzero?`).
p 5.nonzero?
p 0.nonzero?
p(-3.nonzero?)
p 3.5.nonzero?
p 0.0.nonzero?
p 5.nonzero? ? "kept" : "nil"
