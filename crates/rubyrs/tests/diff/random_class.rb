# Tier 1 seeded `Random` class — Mulberry32 PRNG.
#
# Per ADR 0017 row 131 (Random / SecureRandom), the seeded mode
# lives in Tier 1; the unseeded entropy form belongs out. rubyrs
# uses Mulberry32 rather than CRuby's Mersenne Twister, so RAW
# OUTPUT IS NOT byte-identical between the two. This fixture
# uses property-based assertions (range, length, determinism)
# rather than exact-value comparison — both rubyrs and CRuby
# return identical `true` / `false` answers for the assertions
# below, so the diff_cruby harness still pins down the contract.

# Wrong-type seed raises TypeError on both implementations.
# (rubyrs additionally raises ArgumentError on `Random.new` with
# no args — CRuby falls through to system entropy. That
# divergence isn't exercised here; a Rust-side embed test pins
# the rubyrs Tier 1 behaviour separately.)
begin
  Random.new("not-int")
rescue TypeError => e
  puts "wrong-type TypeError: ok"
end

# `#seed` returns the original argument.
puts Random.new(0).seed
puts Random.new(42).seed
puts Random.new(-7).seed

# Determinism — same seed produces same sequence.
r1 = Random.new(42)
r2 = Random.new(42)
puts r1.rand(1_000_000) == r2.rand(1_000_000)   # true
puts r1.rand(1_000_000) == r2.rand(1_000_000)   # true
puts r1.rand(1_000_000) == r2.rand(1_000_000)   # true

# Different seeds produce different sequences (overwhelming
# probability — collision in even one of the 5 draws is
# astronomically unlikely).
a = Random.new(1)
b = Random.new(2)
collisions = (1..5).count { a.rand(1_000_000) == b.rand(1_000_000) }
puts collisions < 5                             # true (at least one differs)

# `rand` with no arg → Float in [0.0, 1.0).
r = Random.new(0)
f1 = r.rand
f2 = r.rand
puts f1 >= 0.0 && f1 < 1.0                      # true
puts f2 >= 0.0 && f2 < 1.0                      # true

# `rand(n)` with Integer arg → Integer in 0...n.
r = Random.new(123)
1000.times do
  v = r.rand(50)
  raise "out of range: #{v}" unless v.is_a?(Integer) && v >= 0 && v < 50
end
puts "rand(50) range: ok"

# `rand(Float)` → Float in [0.0, arg).
r = Random.new(456)
1000.times do
  v = r.rand(2.5)
  raise "out of range: #{v}" unless v.is_a?(Float) && v >= 0.0 && v < 2.5
end
puts "rand(2.5) range: ok"

# `rand(Range)` — Integer endpoints.
r = Random.new(789)
1000.times do
  v = r.rand(10..20)
  raise "out of range: #{v}" unless v.is_a?(Integer) && v >= 10 && v <= 20
end
puts "rand(10..20) inclusive: ok"

# `rand(Range)` — exclusive end.
r = Random.new(789)
1000.times do
  v = r.rand(10...20)
  raise "out of range: #{v}" unless v.is_a?(Integer) && v >= 10 && v < 20
end
puts "rand(10...20) exclusive: ok"

# Invalid args.
r = Random.new(0)
[0, -1, -100].each do |bad|
  begin
    r.rand(bad)
    puts "expected ArgumentError for #{bad}"
  rescue ArgumentError
    # ok
  end
end
puts "negative-arg ArgumentError: ok"

# `bytes(n)` — returns binary String of exactly n bytes.
r = Random.new(0)
b = r.bytes(16)
puts b.class.name                               # "String"
puts b.bytesize                                 # 16
puts r.bytes(0).bytesize                        # 0

# Different sizes work — full coverage of the trailing-chunk
# truncation path inside Random#bytes.
r = Random.new(0)
[1, 2, 3, 4, 5, 17, 32, 100].each do |n|
  raise "size #{n} mismatch" unless r.bytes(n).bytesize == n
end
puts "bytes sizing: ok"

# Negative bytes — ArgumentError.
begin
  Random.new(0).bytes(-1)
rescue ArgumentError
  puts "negative bytes ArgumentError: ok"
end
