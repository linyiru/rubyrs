# Smallest practical Ruby program — measured by `perf/wasm_check.sh`
# as the wasmtime startup floor (NOT a literal cold-cache cold start;
# see `perf/wasm_baselines.tsv` for why min-of-3 is the steady-state
# floor, not the first-run cold-cache time).
#
# Has minimal allocations and minimal dispatch — `puts` is still
# Kernel#puts and "ok" still constructs a String. The intent is "as
# small a workload as you can usefully write in Ruby"; the wall time
# is dominated by wasmtime startup + rubyrs runtime init, with the
# script body itself in the noise.
puts "ok"
