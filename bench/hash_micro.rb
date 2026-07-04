# Small-Hash micro benchmark — pure-Ruby Hash construction / lookup /
# delete / iteration, rubyrs vs CRuby. Written for the record-shape
# Hash-allocation investigation (JSON parse_sids gap): answers "is the
# small-hash cost JSON-specific or a general VM cost?".
#
# Run on both runtimes; every op prints ns/op (min of RUNS):
#   ruby bench/hash_micro.rb
#   target/release/rubyrs bench/hash_micro.rb
#
# Cross-runtime constraints honored: no printf (sprintf+puts), no
# Process.clock_gettime (Time.now), no Benchmark stdlib.

N = Integer(ENV["N"] || "200000")
RUNS = Integer(ENV["RUNS"] || "5")

def bench(label, n, runs)
  best = nil
  runs.times do
    t0 = Time.now
    yield n
    ms = (Time.now - t0) * 1000.0
    best = ms if best.nil? || ms < best
  end
  ns = (best * 1_000_000.0) / n
  puts sprintf("%-28s %10.1f ns/op", label, ns)
  ns
end

label = defined?(JSON) ? "?" : nil
puts "runtime: #{RUBY_VERSION rescue "?"} #{defined?(RUBY_DESCRIPTION) ? RUBY_DESCRIPTION : ""}"
puts "N=#{N} RUNS=#{RUNS}"
puts ""

# Baseline loop overhead (subtract mentally; printed, not netted).
bench("empty times loop", N, RUNS) { |n| n.times { } }

# -- Construction ------------------------------------------------
bench("lit 5 string keys", N, RUNS) do |n|
  n.times { { "a" => 1, "b" => 2, "c" => 3, "d" => 4, "e" => 5 } }
end

bench("lit 5 symbol keys", N, RUNS) do |n|
  n.times { { a: 1, b: 2, c: 3, d: 4, e: 5 } }
end

bench("lit 1 string key", N, RUNS) do |n|
  n.times { { "sid" => 1 } }
end

bench("lit 2 string keys", N, RUNS) do |n|
  n.times { { "sid" => "x", "n" => 1 } }
end

bench("empty hash literal", N, RUNS) do |n|
  n.times { {} }
end

bench("Hash[] 5 string keys", N, RUNS) do |n|
  n.times { Hash["a", 1, "b", 2, "c", 3, "d", 4, "e", 5] }
end

KEYS5 = ["a", "b", "c", "d", "e"]
bench("each_with_object 5 sets", N, RUNS) do |n|
  n.times { KEYS5.each_with_object({}) { |k, h| h[k] = 1 } }
end

# -- Lookup (5-key hash, no construction inside the loop) --------
H5S = { "a" => 1, "b" => 2, "c" => 3, "d" => 4, "e" => 5 }
H5Y = { a: 1, b: 2, c: 3, d: 4, e: 5 }

bench("lookup str key hit", N, RUNS) { |n| n.times { H5S["c"] } }
bench("lookup str key miss", N, RUNS) { |n| n.times { H5S["z"] } }
bench("lookup sym key hit", N, RUNS) { |n| n.times { H5Y[:c] } }

# -- merge / merge! (plain keys) — gates the merge-family bulk path
#    (the user-key funnel must not tax plain option-hash merges) ----
MSRC = { "m1" => 1, "m2" => 2, "m3" => 3 }
bench("merge 5<-3 str", N, RUNS) do |n|
  n.times { H5S.merge(MSRC) }
end

bench("merge! 2<-3 str", N, RUNS) do |n|
  n.times { { "x" => 1, "y" => 2 }.merge!(MSRC) }
end

# -- Delete + reinsert cycle on a live 5-key hash ----------------
HD = { "a" => 1, "b" => 2, "c" => 3, "d" => 4, "e" => 5 }
bench("delete+reinsert str", N, RUNS) do |n|
  n.times { HD.delete("c"); HD["c"] = 3 }
end

# -- Iteration ----------------------------------------------------
bench("each over 5 pairs", N, RUNS) do |n|
  s = 0
  n.times { H5S.each { |k, v| s += v } }
end

# -- Growth boundary reference: 24-key literal (above any small-
#    hash threshold) constructed from an inline literal ----------
bench("lit 24 string keys", N / 4, RUNS) do |n|
  n.times do
    { "k00" => 0, "k01" => 1, "k02" => 2, "k03" => 3, "k04" => 4,
      "k05" => 5, "k06" => 6, "k07" => 7, "k08" => 8, "k09" => 9,
      "k10" => 10, "k11" => 11, "k12" => 12, "k13" => 13, "k14" => 14,
      "k15" => 15, "k16" => 16, "k17" => 17, "k18" => 18, "k19" => 19,
      "k20" => 20, "k21" => 21, "k22" => 22, "k23" => 23 }
  end
end
