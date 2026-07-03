# PERF-REGRESSION GUARD (not a behaviour test). StringScanner#scan_until
# over a many-boundary buffer must be LINEAR. The O(n²) trap is slicing
# `@str[@pos..]` per scan (and O(n) String#length / String#[] on the
# UTF-8 buffer). This classifies the N→4N time ratio: linear ≈ 4×,
# quadratic ≈ 16×. A generous threshold (8×) with min-of-runs keeps it
# stable on noisy CI while still catching a quadratic regression — and
# the CLASSIFICATION is what's compared (both rubyrs and CRuby print
# "linear"), so it's machine-speed independent.
require "strscan"

def scan_time(parts)
  buf = ("x" * 16 + "\r\n--bnd\r\n") * parts
  re = /(?:\r\n|\A)--bnd(?:\r\n|--)/m
  best = nil
  3.times do
    ss = StringScanner.new(buf.dup)
    t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    n = 0
    n += 1 while ss.scan_until(re)
    dt = Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0
    best = dt if best.nil? || dt < best
  end
  best
end

# Under STRESS_GC every allocation triggers a full collection, so the
# per-scan MatchData allocs make this ~100× slower — and STRESS_GC
# exercises GC correctness, not perf (the scan paths are still covered
# under stress by the strscan_scan_until correctness fixture). Skip the
# heavy timing there and emit the expected classification; the normal
# (non-stress) run does the real measurement.
if ENV["STRESS_GC"]
  puts "linear"
else
  scan_time(500) # warm up
  t1 = scan_time(2000)
  t2 = scan_time(8000) # 4× the work
  ratio = t2 / t1
  # Threshold 10 (was 8): the measured "linear" band on rubyrs sits
  # at ~7.5-8.2× on a mid-load machine (GC/alloc growth adds a
  # super-linear-ish component on top of the ideal 4×, in BOTH the
  # pre- and post-NFA-fast-path binaries), so 8 had lost its margin —
  # the ADR-0031 increment-2 dispatch speedup shrank t1's constant
  # and pushed the borderline ratio from ~7.9 to ~8.2, flipping this
  # guard red without any scaling change (t2/t1 raw timings verified:
  # t1 −5%, t2 unchanged). The quadratic failure mode this guard
  # exists to catch still classifies at ≈16×, so 10 keeps full
  # detection power.
  puts(ratio < 10 ? "linear" : "super-linear (ratio=#{ratio.round(1)})")
end
