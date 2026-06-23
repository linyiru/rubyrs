# Hash#merge (Ruby 3.0+) takes zero or more hashes, applied left-to-right.
p({a: 1}.merge({b: 2}))
p({a: 1}.merge({b: 2}, {c: 3}))
p({a: 1, b: 1}.merge({b: 2}, {b: 3, c: 4}))
p({a: 1}.merge)
p({a: 1}.merge({a: 2}) { |k, o, n| o + n })
p({}.merge({x: 1}, {y: 2}, {x: 9}))
