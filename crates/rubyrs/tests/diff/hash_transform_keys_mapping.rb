# Hash#transform_keys(mapping_hash) (Ruby 2.5+): keys present in the
# mapping are replaced by the mapped value; absent keys are kept (or,
# when a block is also given, passed through the block). Last-wins on
# key collision, iteration order preserved.
p({a: 1, b: 2, c: 3}.transform_keys({a: :x, b: :y}))
p({a: 1}.transform_keys({z: :q}))
p({a: 1, b: 2}.transform_keys({a: :dup, b: :dup}))
p({"x" => 1, "y" => 2}.transform_keys({"x" => "X"}))
p({a: 1, b: 2, c: 3}.transform_keys({}))

# With a block: mapping wins; only unmapped keys hit the block.
p({a: 1, b: 2}.transform_keys({a: :x}) { |k| k.to_s.upcase })
p({a: 1, b: 2, c: 3}.transform_keys({b: :B}) { |k| k.to_s })
p({a: 1, b: 2}.transform_keys({a: :x, b: :y}) { |k| "unused" })

# Plain block form (no mapping) unaffected.
p({a: 1, b: 2}.transform_keys { |k| k.to_s })
