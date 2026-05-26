# Smallest possible Ruby program — exercises just the parse-and-
# print path. Used by `perf/wasm_check.sh` to fence the wasmtime
# cold-start budget; this script intentionally has no allocations,
# no method dispatch, and no GC pressure so the wall time is
# dominated by wasmtime startup + rubyrs runtime init. Kept
# tiny so the budget can land near the wasmtime floor and any
# regression shows up loudly.
puts "ok"
