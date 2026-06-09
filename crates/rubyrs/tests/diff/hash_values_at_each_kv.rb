# Hash#values_at, #each_key, #each_value (block + no-block Enumerator).
p({ a: 1, b: 2, c: 3 }.values_at(:a, :c))
p({ a: 1, b: 2 }.values_at(:a, :x))      # miss → nil
p({ a: 1, b: 2 }.values_at)              # no keys → []
p(Hash.new(0).values_at(:x, :y))         # default value
r = []; { a: 1, b: 2 }.each_key { |k| r << k }; p r
r = []; { a: 1, b: 2 }.each_value { |v| r << v }; p r
p({ a: 1, b: 2 }.each_key.to_a)
p({ a: 1, b: 2 }.each_value.to_a)
p({ a: 1, b: 2 }.each_key { |k| })       # returns self
p({ a: 1, b: 2 }.each_value.map { |v| v * 10 })
p({ a: 1, b: 2 }.each_value { |v| break v if v == 2 })
p({}.respond_to?(:values_at))
p({}.respond_to?(:each_key))
p({}.respond_to?(:each_value))
