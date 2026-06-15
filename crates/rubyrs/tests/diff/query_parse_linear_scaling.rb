# PERF-REGRESSION GUARD + light correctness for the query-parser core.
# rack's QueryParser#parse_query / parse_nested_query splits the query on
# "&", splits each pair on "=", and URI.decode_www_form_component's both
# halves into a Hash. That whole loop must be LINEAR in the number of
# pairs (a 128 MB urlencoded POST body is a real rack input; the native
# URI decode plus linear String ops keep it linear). This classifies the
# N->4N time ratio (linear ~= 4x, quadratic ~= 16x; threshold 8x with
# min-of-runs); the CLASSIFICATION is compared, so both interpreters
# print "linear" and a quadratic regression makes rubyrs print
# "super-linear" -> diff fails. STRESS_GC skips the heavy timing.
require "uri"

def parse_query(q)
  h = {}
  q.split("&").each do |pair|
    k, v = pair.split("=", 2)
    h[URI.decode_www_form_component(k)] = URI.decode_www_form_component(v || "")
  end
  h
end

# light correctness: percent- and plus-decoding, "=" in value, missing value
p parse_query("a=1&b%20c=x%2By&d&e=f=g")

if ENV["STRESS_GC"]
  puts "linear"
else
  def timed(n)
    q = (0...n).map { |i| "k#{i}=v%20#{i}" }.join("&")
    best = nil
    3.times do
      t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      parse_query(q)
      dt = Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0
      best = dt if best.nil? || dt < best
    end
    best
  end
  timed(500) # warm up
  t1 = timed(2000)
  t2 = timed(8000) # 4x the work
  ratio = t2 / t1
  puts(ratio < 8 ? "linear" : "super-linear (ratio=#{ratio.round(1)})")
end
