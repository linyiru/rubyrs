# Instrumented-key hash/eql? call PATTERNS on small hashes — the parts
# of CRuby's ar_table behavior a program can observably depend on, and
# which rubyrs's user-key index reproduces exactly (probed 2026-07,
# CRuby 3.4):
#   - every insert calls key.hash (a wrong-arity override raises there)
#   - a value-equal duplicate insert calls eql? ONLY against the
#     stored equal key (hash prefilter skips non-colliding keys),
#     updates in place, keeps size
#   - a lookup hit calls hash once + eql? once against the hit
#   - a lookup miss calls hash once and NO eql? (holds only because
#     the keys' hash values are deterministic and distinct mod 256 —
#     see the note inside K#hash)
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
    # Return the RAW integer, not @val.hash: CRuby seeds Integer#hash
    # per process, and its packed ar_table compares only a 1-BYTE hint
    # before calling eql? — seeded hashes therefore roll a ~1/256
    # per-pair-per-process chance of a phantom eql? on ANY probe
    # (this fixture flaked exactly that way on CI, 2026-07-05).
    # Custom-hash RETURN VALUES are used unmixed (probed: 0 phantom
    # eql? in 400 seeded processes with raw ints), so distinct-mod-256
    # @vals make every line deterministic on both engines while still
    # exercising the hash-prefilter contract.
    @val
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
