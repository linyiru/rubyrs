# Hash#transform_values / #transform_keys with NO block return an
# Enumerator; driving it with a block re-runs the transform and
# returns the resulting Hash.
h = {a: 1, b: 2, c: 3}
p h.transform_values.class
p h.transform_keys.class
p h.transform_values.with_index { |v, i| [v, i] }
p h.transform_keys.with_index { |k, i| "#{k}#{i}" }
p h.transform_values.each_with_index.to_a
# Block / proc forms unaffected.
p h.transform_values { |v| v * 10 }
p h.transform_keys(&:to_s)
