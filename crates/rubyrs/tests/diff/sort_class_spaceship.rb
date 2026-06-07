# Array#sort / #sort! / #sort_by! use a value's `<=>` — including a
# class-level `def self.<=>` (so sorting Class objects works), and
# raise ArgumentError (not NoMethodError) on incomparable elements.
class Plugin
  PRI = { high: 10, normal: 20, low: 30 }
  def self.prio; :normal; end
  def self.<=>(other); PRI[other.prio] <=> PRI[prio]; end
end
class A < Plugin; def self.prio; :high; end; end
class B < Plugin; def self.prio; :low; end; end
p([B, A].sort.map(&:name))
p([B, A].sort_by { |c| Plugin::PRI[c.prio] }.map(&:name))
begin; [Object.new, Object.new].sort; rescue => e; p e.class; end
begin; ["a", 1].sort; rescue => e; p e.class; end
# sort_by! mutates in place and returns self
arr = [3, 1, 2]; sr = arr.sort_by! { |x| -x }; p [sr, arr, arr.equal?(sr)]
words = ["bb", "a", "ccc"]; words.sort_by!(&:length); p words
