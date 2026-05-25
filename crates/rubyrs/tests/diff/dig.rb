# Hash#dig / Array#dig — nested data access with nil-safety.
# Any missing key/index short-circuits to nil.

# Hash#dig — simple nested.
h = {a: {b: {c: 42}}}
p h.dig(:a)
p h.dig(:a, :b)
p h.dig(:a, :b, :c)

# Missing key at any level → nil.
p h.dig(:a, :b, :missing)
p h.dig(:a, :x, :c)
p h.dig(:nope)

# Array#dig — by integer index, supports negative.
a = [[1, 2], [3, [4, 5]]]
p a.dig(0)
p a.dig(0, 1)
p a.dig(1, 1, 0)
p a.dig(-1, -1, -1)
p a.dig(1, 1, 10)
p a.dig(100)

# Mixed Hash + Array nesting.
mixed = {users: [{name: "alice", age: 30}, {name: "bob", age: 25}]}
p mixed.dig(:users, 0, :name)
p mixed.dig(:users, 0, :age)
p mixed.dig(:users, 1, :name)
p mixed.dig(:users, 5, :name)
p mixed.dig(:users, 0, :missing)
p mixed.dig(:missing, 0)

# Mix in nil-safe chain patterns.
config = {
  db: {
    host: "localhost",
    port: 5432,
    creds: nil,  # explicit nil
  },
}
p config.dig(:db, :host)
p config.dig(:db, :creds, :user)
p config.dig(:db, :unknown)
p config.dig(:cache, :ttl)

# Inside a class.
class Lookup
  def initialize(data)
    @data = data
  end
  def get(*keys)
    @data.dig(*keys)
  end
end

l = Lookup.new(mixed)
p l.get(:users, 0, :name)
p l.get(:users, 999, :name)

# Dig with mixed key types.
hybrid = {list: [10, 20, 30], pairs: {x: [:a, :b]}}
p hybrid.dig(:list, 1)
p hybrid.dig(:pairs, :x, 0)
p hybrid.dig(:pairs, :x, 5)
