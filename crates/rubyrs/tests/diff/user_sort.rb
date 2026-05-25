# Array#sort / Array#sort_by — with user classes whose `<=>` is
# defined manually (typically via `include Comparable`).
# Built-in element types still use the value-cmp fast path.

class Ver
  include Comparable
  attr_reader :n
  def initialize(n); @n = n; end
  def <=>(o); @n <=> o.n; end
  def to_s; "v#{@n}"; end
end

# Plain sort with user objects.
arr = [Ver.new(3), Ver.new(1), Ver.new(2), Ver.new(0), Ver.new(5)]
puts arr.sort.map { |v| v.to_s }.inspect

# Reverse via sort_by.
puts arr.sort_by { |v| -v.n }.map { |v| v.to_s }.inspect

# Sort by a custom-class key.
class Person
  attr_reader :name, :age
  def initialize(name, age); @name = name; @age = age; end
end

people = [
  Person.new("Charlie", 30),
  Person.new("Alice",   25),
  Person.new("Bob",     35),
]
by_age = people.sort_by { |p| p.age }
puts by_age.map { |p| "#{p.name}:#{p.age}" }.inspect
by_name = people.sort_by { |p| p.name }
puts by_name.map { |p| p.name }.inspect

# sort_by with a Ver key (key class also uses Comparable).
records = [Ver.new(3), Ver.new(1), Ver.new(2)]
keyed = records.sort_by { |v| v }    # Ver itself as the key
puts keyed.map { |v| v.to_s }.inspect

# Stability of sort_by — equal keys preserve insertion order.
class Tagged
  attr_reader :k, :id
  def initialize(k, id); @k = k; @id = id; end
end
items = [
  Tagged.new(1, "a"),
  Tagged.new(2, "b"),
  Tagged.new(1, "c"),
  Tagged.new(2, "d"),
  Tagged.new(1, "e"),
]
stable = items.sort_by { |t| t.k }
puts stable.map { |t| "#{t.k}:#{t.id}" }.inspect

# Built-in fast path still works (Int/Str/Sym).
puts [3, 1, 4, 1, 5, 9, 2, 6, 5].sort.inspect
puts ["banana", "apple", "cherry"].sort.inspect
puts [:z, :a, :m].sort.inspect

# Empty array.
puts [].sort.inspect
puts [].sort_by { |x| x }.inspect

# Single element.
puts [42].sort.inspect

# sort_by with negative-key trick for descending sort.
puts [1, 2, 3, 4, 5].sort_by { |n| -n }.inspect

# sort_by inside a class method.
class Inventory
  def initialize(items)
    @items = items
  end
  def by_count
    @items.sort_by { |pair| -pair[1] }.map { |pair| pair[0] }
  end
end

inv = Inventory.new([["apple", 3], ["banana", 7], ["cherry", 1]])
puts inv.by_count.inspect

# sort with a class that mixes in Comparable but only handles
# same-type pairs (incomparable → nil → sort returns None,
# which we surface as NoMethodError matching CRuby).
class Strict
  include Comparable
  attr_reader :n
  def initialize(n); @n = n; end
  def <=>(o)
    return nil if o.class.name != "Strict"
    @n <=> o.n
  end
end

ss = [Strict.new(2), Strict.new(1), Strict.new(3)]
puts ss.sort.map { |s| s.n }.inspect
