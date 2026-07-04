# INCONSISTENT user keys — eql? returns true but each instance hashes
# DIFFERENTLY: CRuby never eql?-compares hash-distinct keys (the table
# prefilters by hash), so such keys legitimately DUPLICATE at every
# entry point. The dedup scanners must bucket by the key's Ruby #hash
# and only eql?-compare within a bucket — never pairwise-eql? across
# the board. (Adversarial-verifier probes 20_inconsistent /
# 24_hash_groupby_badk, 2026-07.)

class BadK
  @@ctr = 0
  def hash = (@h ||= (@@ctr += 1))
  def eql?(o) = o.is_a?(BadK)
  def ==(o) = eql?(o)
  def inspect = "BadK"
end

h = { BadK.new => :a, BadK.new => :b }
puts "lit: #{h.size}"
h2 = Hash[BadK.new, :a, BadK.new, :b]
puts "Hash[]: #{h2.size}"
h3 = Hash[[[BadK.new, :a], [BadK.new, :b]]]
puts "Hash[pairs]: #{h3.size}"
h4 = { x: 1, y: 2 }
h4.transform_keys! { BadK.new }
puts "tk!: #{h4.size}"
h5 = { x: 1, y: 2 }.transform_keys { BadK.new }
puts "tk: #{h5.size}"
h6 = { BadK.new => 1 }.merge({ BadK.new => 2 })
puts "merge: #{h6.size}"
h7 = { BadK.new => 1 }
h7.merge!({ BadK.new => 2 })
puts "merge!: #{h7.size}"
h8 = [[BadK.new, 1], [BadK.new, 2]].to_h
puts "to_h: #{h8.size}"
h9 = { a: BadK.new, b: BadK.new }.invert
puts "invert: #{h9.size}"
g = [1, 2].group_by { BadK.new }
puts "group_by: #{g.size}"
gh = { x: 1, y: 2 }.group_by { BadK.new }
puts "hash-group_by: #{gh.size}"
t = [BadK.new, BadK.new].tally
puts "tally: #{t.size}"
s1 = { **{ BadK.new => 1 }, **{ BadK.new => 2 } }
puts "splat: #{s1.size}"
e = [1, 2].each_with_object({}) { |i, acc| acc[BadK.new] = i }
puts "ewo: #{e.size}"
puts "==: #{{ BadK.new => 1 } == { BadK.new => 1 }}"

# CONSISTENT user keys still dedup at the same entry points (guard the
# guard: the prefilter must not stop equal-hash keys from comparing)
class GoodK
  attr_reader :v
  def initialize(v) = @v = v
  def hash = @v.hash
  def eql?(o) = o.is_a?(GoodK) && o.v == @v
  def inspect = "GoodK(#{@v})"
end
puts "good-lit: #{{ GoodK.new(1) => :a, GoodK.new(1) => :b }.size}"
puts "good-Hash[]: #{Hash[GoodK.new(2), :a, GoodK.new(2), :b].size}"
gk = { x: 1, y: 2 }
gk.transform_keys! { GoodK.new(3) }
puts "good-tk!: #{gk.size}"
puts "good-hgroup: #{{ x: 1, y: 2 }.group_by { GoodK.new(4) }.size}"

# O(n) sanity: many distinct user keys through the scratch builders
# (pre-fix these were O(n^2) eql? dispatches; here they must both
# complete fast AND stay correct)
class NK
  attr_reader :v
  def initialize(v) = @v = v
  def hash = @v.hash
  def eql?(o) = o.is_a?(NK) && o.v == @v
end
src = {}
600.times { |i| src[i] = i }
tk = src.dup
tk.transform_keys! { |k| NK.new(k) }
puts "tk-600: #{tk.size}"
gg = src.group_by { |k, v| NK.new(k % 300) }
puts "group-600: #{gg.size} #{gg.values.map(&:size).sum}"
