# Hash#rehash — added with the 2026-07 small-hash representation work.
# CRuby recomputes stored key hashes; rubyrs (which content-scans small
# hashes and rebuilds its lazy index from live content) reproduces the
# observable post-rehash state: keys that have BECOME eql? collapse with
# the FIRST key object keeping its position and the LAST value winning
# (probed on CRuby 3.4 — same rule as duplicate-key literals).

puts "== mutated array key becomes findable after rehash =="
ak = [1, 2]
h = { ak => :v, "x" => 1 }
ak << 3
h.rehash
p h[[1, 2, 3]]
p h.keys

puts "== keys that became equal collapse: first position, last value =="
a = [1]
b = [2]
h2 = { "first" => 0, a => :va, "mid" => 1, b => :vb, "last" => 2 }
a[0] = 2 # now a == b
h2.rehash
p h2.keys
p h2.values
puts "survivor is the first key object: #{h2.keys[1].equal?(a)}"

puts "== rehash returns self =="
h3 = { "a" => 1 }
puts h3.rehash.equal?(h3)

puts "== rehash on a large (indexed) hash =="
h4 = {}
(1..20).each { |i| h4[[i]] = i }
h4.keys.first << 99 # mutate a key in place
h4.rehash
puts h4[[1, 99]]
puts h4.size

puts "== rehash with user eql?/hash keys =="
class Uk
  attr_reader :v
  def initialize(v)
    @v = v
  end
  def hash
    @v.hash
  end
  def eql?(other)
    other.is_a?(Uk) && other.v == @v
  end
end
u1 = Uk.new(1)
u2 = Uk.new(2)
h5 = { u1 => :one, u2 => :two }
u2.instance_variable_set(:@v, 1) # u2 becomes eql? to u1
h5.rehash
puts "size after collapse: #{h5.size} value: #{h5[Uk.new(1)].inspect}"

puts "== frozen hash raises =="
begin
  { "a" => 1 }.freeze.rehash
rescue FrozenError
  puts "FrozenError"
end
