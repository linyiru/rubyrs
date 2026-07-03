# JSON benchmark — same workload on rubyrs (pure canon),
# rubyrs (--features _json_native), and CRuby (stdlib).
#
# Workload: parse + generate over a synthetic 100-key Object
# (mix of String / Integer / Float / Bool / Null + a nested
# 20-element Array of Hashes), N iterations per measurement.
#
# Measurement: Process.clock_gettime(Process::CLOCK_MONOTONIC)
# pairs around each loop; report ms wall + ms-per-iteration.
# Three runs per metric so the printed minimum absorbs warm-up
# / GC noise without needing a stats lib.
#
# Run:
#   ruby bench/json_bench.rb              # CRuby + stdlib JSON
#   target/release/rubyrs bench/json_bench.rb  # rubyrs pure canon
#                                              # OR _json_native
#                                              # (auto-detected)
#
# The runtime label printed identifies which line is which when
# comparing logs.
require "json"
# Oj is opt-in via env knob: BENCH_OJ=1. CRuby-only gem (C
# extension); rubyrs doesn't load it because flori_native pattern
# applies (different gem, different mode-driven shape). When set,
# the script runs an extra column alongside JSON.parse /
# JSON.generate using `Oj.load(s, mode: :strict)` /
# `Oj.dump(obj, mode: :strict)` — that's the apples-to-apples
# subset (no Ruby Object encoding, no extras). On rubyrs the
# require is skipped silently (no NameError) so the same script
# stays cross-runtime runnable.
HAVE_OJ = begin
  require "oj"
  true
rescue LoadError, RuntimeError
  # rubyrs raises RuntimeError ("cannot find C ext: oj") rather
  # than LoadError when a cext gem isn't loadable in its
  # sandboxed environment; catch both so the script stays
  # cross-runtime runnable.
  false
end

ITERS = Integer(ENV["ITERS"] || "5000")
RUNS = Integer(ENV["RUNS"] || "3")

# Build payload once; same bytes go into every run so the
# parse-side workload is identical across implementations.
def build_payload
  inner = (1..20).map do |i|
    {
      "id" => i,
      "name" => "item-#{i}",
      "active" => i.even?,
      "score" => i * 1.5,
      "tags" => ["alpha", "beta", "gamma"],
    }
  end
  payload = {}
  100.times do |i|
    payload["key_#{i}"] = case i % 5
    when 0 then i
    when 1 then "string-#{i}"
    when 2 then i.to_f / 7.0
    when 3 then (i % 2 == 0)
    else nil
    end
  end
  payload["items"] = inner
  payload
end

OBJ = build_payload
JSON_BYTES = JSON.generate(OBJ)
# Snowflake/Stripe-ID shaped payload: long digit runs INSIDE string
# values. Guards the bigint pre-scan's string-awareness — a context-
# blind scan declined these documents to the ~200x-slower pure canon
# (a measured 160x parse regression, fixed 2026-07).
SID_BYTES = JSON.generate((1..200).map { |i| { "sid" => (1234567890123456789 + i).to_s, "n" => i } })

runtime_label = if defined?(JSON::NATIVE_AVAILABLE)
  JSON::NATIVE_AVAILABLE ? "rubyrs (_json_native)" : "rubyrs (pure canon)"
else
  "CRuby #{RUBY_VERSION} (stdlib json)"
end

puts "runtime: #{runtime_label}"
puts "iters:   #{ITERS}  runs: #{RUNS}"
puts "payload: #{JSON_BYTES.bytesize} bytes"
puts ""

def time_ms
  # `Time.now` is available on both CRuby and rubyrs without
  # extra requires. rubyrs lacks `Process.clock_gettime`, and
  # `Time.now` is wall-clock-monotonic enough at the scales
  # we measure (multi-ms loops). The subtraction returns
  # Float seconds; × 1000 → ms.
  t0 = Time.now
  yield
  ((Time.now - t0) * 1000.0).to_f
end

def bench(label, runs, iters)
  best = nil
  runs.times do
    ms = time_ms { iters.times { yield } }
    best = ms if best.nil? || ms < best
  end
  per_iter_us = (best * 1000.0) / iters
  # `printf` isn't on rubyrs; build the line via sprintf + puts
  # (both are available on both runtimes).
  line = sprintf("%-22s  best_total=%9.2f ms   per_iter=%9.3f us",
    label, best, per_iter_us)
  puts line
end

bench("parse",            RUNS, ITERS) { JSON.parse(JSON_BYTES) }
bench("generate",         RUNS, ITERS) { JSON.generate(OBJ) }
bench("round_trip",       RUNS, ITERS) { JSON.generate(JSON.parse(JSON_BYTES)) }
bench("parse_sids",       RUNS, ITERS) { JSON.parse(SID_BYTES) }

if HAVE_OJ
  puts ""
  puts "-- Oj (mode: :strict) ----------"
  bench("oj_parse",       RUNS, ITERS) { Oj.load(JSON_BYTES, mode: :strict) }
  bench("oj_generate",    RUNS, ITERS) { Oj.dump(OBJ, mode: :strict) }
  bench("oj_round_trip",  RUNS, ITERS) { Oj.dump(Oj.load(JSON_BYTES, mode: :strict), mode: :strict) }
end
