# Instrumented-key hash/eql? call PATTERNS on small hashes — the parts
# of CRuby's ar_table behavior a program can observably depend on, and
# which rubyrs's user-key index reproduces exactly (probed 2026-07,
# CRuby 3.4):
#   - every insert calls key.hash (a wrong-arity override raises there)
#   - a value-equal duplicate insert calls eql? ONLY against the
#     stored equal key (hash prefilter skips non-colliding keys),
#     updates in place, keeps size
#   - a lookup hit calls hash once + eql? once against the hit
#   - a lookup miss calls hash once and NO eql?
# (The aggregate hash-call COUNT across CRuby's 8->9 ar->st conversion
# — where CRuby re-hashes the existing keys — is NOT pinned here; that
# is a table-conversion artifact, not a per-op contract.)

$calls = []
class K
  attr_reader :tag
  def initialize(tag, val)
    @tag = tag
    @val = val
  end
  def hash
    $calls << [:hash, @tag]
    @val.hash
  end
  def eql?(other)
    $calls << [:eql?, @tag, other.tag]
    other.is_a?(K) && @val == other.instance_variable_get(:@val)
  end
end

h = {}
a = K.new(:a, 1)
b = K.new(:b, 2)

$calls = []
h[a] = 10
puts "insert a:        #{$calls.inspect}"

$calls = []
h[b] = 20
puts "insert b:        #{$calls.inspect}"

$calls = []
h[K.new(:a2, 1)] = 30
puts "dup-insert a2:   #{$calls.inspect}"
puts "in-place update: size=#{h.size} value=#{h[a]}"

$calls = []
h[K.new(:probe, 2)]
puts "lookup hit:      #{$calls.inspect}"

$calls = []
h.key?(K.new(:miss, 99))
puts "lookup miss:     #{$calls.inspect}"

# The per-op patterns hold identically PAST the small-table boundary.
h2 = {}
keys = (0...9).map { |i| K.new("k#{i}".to_sym, 100 + i) }
keys.each_with_index { |k, i| h2[k] = i }
$calls = []
h2[K.new(:probe9, 104)]
puts "9-key lookup:    #{$calls.inspect}"
$calls = []
h2.key?(K.new(:miss9, 999))
puts "9-key miss:      #{$calls.inspect}"

# wrong-arity hash override raises on insert (CRuby calls key.hash there)
class BadHash
  def hash(_extra)
    0
  end
end
begin
  { BadHash.new => 1 }
rescue ArgumentError => e
  puts "wrong-arity hash: ArgumentError"
end
