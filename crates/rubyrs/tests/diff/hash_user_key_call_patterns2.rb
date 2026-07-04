# Second instrumented call-pattern battery (companion to
# hash_instrumented_key_calls.rb) — pins the ORIENTATION of dedup eql?
# dispatches and the no-stored-rehash property (probed CRuby 3.4):
#   - literal / Hash[] dedup: eql? receiver is the LATER (incoming)
#     key, the argument the stored one
#   - merge! into a literal-built hash only hashes the INCOMING key —
#     stored keys' hashes were computed at build time and are reused
#   - h1 == h2 hashes only h1's (query) keys; h2's stored keys are not
#     re-hashed
# Call LISTS are uniq-normalized: CRuby's st internals may repeat a
# dispatch (e.g. transform_keys! collision fires eql? twice); the
# CONTRACT is which pairs ever compare and which keys ever re-hash,
# not internal retry counts.

$eqls = []
$hashes = []
class OK
  attr_reader :t
  def initialize(t, v) = (@t = t; @v = v)
  def hash = ($hashes << @t; @v.hash)
  def eql?(o) = ($eqls << [@t, o.t]; o.is_a?(OK) && o.instance_variable_get(:@v) == @v)
end

# literal dedup orientation
$eqls = []
h = { OK.new(:stored, 1) => 1, OK.new(:incoming, 1) => 2 }
puts "lit-eqls:    #{$eqls.uniq.inspect} size=#{h.size}"

# Hash[] dedup orientation
$eqls = []
h2 = Hash[OK.new(:hstored, 2), 1, OK.new(:hincoming, 2), 2]
puts "hashk-eqls:  #{$eqls.uniq.inspect} size=#{h2.size}"

# transform_keys! collision orientation
$eqls = []
src = { x: 1, y: 2 }
ks = [OK.new(:tfirst, 3), OK.new(:tsecond, 3)]
i = -1
src.transform_keys! { i += 1; ks[i] }
puts "tk-eqls:     #{$eqls.uniq.inspect} size=#{src.size}"

# merge! into a literal-built hash: only the incoming key re-hashes
h3 = { OK.new(:s1, 10) => 1, OK.new(:s2, 11) => 2 }
$hashes = []
h3.merge!({ OK.new(:mnew, 12) => 3 })
puts "merge-hash:  #{$hashes.uniq.inspect} size=#{h3.size}"

# == hashes only the query side's keys
h4 = { OK.new(:qa, 20) => 1 }
h5 = { OK.new(:qb, 20) => 1 }
$hashes = []
r = h4 == h5
puts "eq-hash:     #{$hashes.uniq.inspect} #{r}"

# lookup into a literal-built hash: stored keys not re-hashed
h6 = { OK.new(:la, 30) => :v, OK.new(:lb, 31) => :w }
$hashes = []
p h6[OK.new(:lq, 30)]
puts "aref-hash:   #{$hashes.uniq.inspect}"
