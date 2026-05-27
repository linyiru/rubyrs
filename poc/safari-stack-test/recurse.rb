# Binary-search the deepest recursion level the host can sustain.
#
# Each level adds one Ruby call frame plus whatever wasm-level
# frames our interpreter consumes per Ruby call. The first level
# to crash (host stack overflow → wasm trap → JS RangeError)
# is the ceiling.
#
# We probe with `rescue SystemStackError` first so the interpreter
# can report a number even when *it* runs out of frames. If the
# host JS stack blows first, the wasm trap is unrecoverable and
# the harness will report the LAST successful "ok N" line.

def f(n)
  return 0 if n <= 0
  1 + f(n - 1)
end

# Probe levels in increasing powers of 2.
[100, 500, 1000, 2000, 4000, 8000, 16000, 32000, 64000, 128000, 256000, 512000, 1_000_000].each do |n|
  begin
    r = f(n)
    puts "ok #{n} -> #{r}"
  rescue SystemStackError => e
    puts "rubyrs SystemStackError at depth #{n}: #{e.message}"
    break
  rescue => e
    puts "other error at depth #{n}: #{e.class}: #{e.message}"
    break
  end
end
puts "done"
