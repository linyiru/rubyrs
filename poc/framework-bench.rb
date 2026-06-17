# Benchmark CRuby vs rubyrs running real frameworks, end to end.
#
#   ruby poc/framework-bench.rb
#
# Each case launches the interpreter on a probe script N times (the honest way
# to measure boot/require/execute — `require` happens once per process) and
# reports min + median wall-clock. We compare the SAME script on both, scoped to
# the work both complete identically:
#   - Bridgetown: boot + configure (BT_BOOT_ONLY; rubyrs hits a Marshal wall at
#     Site.new that CRuby doesn't).
#   - Hanami::Router: full — boot router, compile routes, serve Rack requests.
require "shellwords"

ROOT     = File.expand_path("..", __dir__)
CRUBY    = ENV["CRUBY"] || "ruby"
RUBYRS   = ENV["RUBYRS"] || File.join(ROOT, "target/release/rubyrs")
WARMUP   = Integer(ENV["WARMUP"] || 2)
RUNS     = Integer(ENV["RUNS"] || 8)

def run_once(cmd, env)
  t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  pid = Process.spawn(env, *cmd, out: File::NULL, err: File::NULL)
  _, status = Process.wait2(pid)
  dt = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1000.0
  [dt, status.success?]
end

def measure(label, cmd, env = {})
  WARMUP.times { run_once(cmd, env) }
  times = []
  ok = true
  RUNS.times do
    dt, success = run_once(cmd, env)
    ok &&= success
    times << dt
  end
  times.sort!
  median = times[times.size / 2]
  { label: label, min: times.first, median: median, ok: ok }
end

CASES = [
  { name: "Bridgetown boot+configure",
    script: "poc/bridgetown-spike/bt-probe.rb", env: { "BT_BOOT_ONLY" => "1" } },
  { name: "Hanami::Router boot+route+serve",
    script: "poc/hanami-spike/hr-probe.rb", env: {} },
]

puts "framework boot benchmark — #{RUNS} runs (+#{WARMUP} warmup), min / median ms"
puts "CRuby:  #{`#{CRUBY} -v`.strip}"
puts "rubyrs: #{RUBYRS}"
puts

CASES.each do |c|
  script = File.join(ROOT, c[:script])
  cru = measure("CRuby",  Shellwords.split(CRUBY)  + [script], c[:env])
  rrs = measure("rubyrs", Shellwords.split(RUBYRS) + [script], c[:env])
  speedup = cru[:median] / rrs[:median]
  puts c[:name]
  printf "  CRuby   : %8.1f ms (min %.1f)%s\n", cru[:median], cru[:min], cru[:ok] ? "" : "  [FAILED]"
  printf "  rubyrs  : %8.1f ms (min %.1f)%s\n", rrs[:median], rrs[:min], rrs[:ok] ? "" : "  [FAILED]"
  printf "  rubyrs is %.2fx %s than CRuby (by median)\n\n", (speedup >= 1 ? speedup : 1 / speedup), (speedup >= 1 ? "FASTER" : "slower")
end
