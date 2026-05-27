# Safari / Chrome wasm stack-depth + throughput PoC

Browser harness backing the "Browser engine variation" section of
[`docs/BENCHMARKS.md`](../../docs/BENCHMARKS.md).

## What it measures

1. **Recursion depth** (`recurse.rb`) — how deep can Ruby recursion go
   before the host engine traps or CRuby raises `SystemStackError`.
   rubyrs uses heap-allocated frames (`Vec<Frame>` in `vm.rs`), so it's
   structurally immune to wasm-stack exhaustion from Ruby recursion;
   CRuby (ruby.wasm) walks the host C stack one frame per Ruby call.

2. **Throughput** (`fizzbuzz_1m.rb`) — 1M-iteration fizzbuzz wall time
   under a fresh WASI instance per iteration. Reports MIN of N runs.

Same script, same harness, same shim — only the wasm binary and the
host browser engine change.

## Run it

```bash
# 1. Build rubyrs.wasm (see DEVELOPMENT.md for WASI-SDK setup):
cargo build --release --target wasm32-wasip1 --no-default-features
cp target/wasm32-wasip1/release/rubyrs.wasm poc/safari-stack-test/

# 2. Drop ruby.wasm 3.4 wasi-minimal next to it (download from
#    https://github.com/ruby/ruby.wasm/releases):
cp ~/ruby-wasm/ruby-3.4-wasm32-unknown-wasip1-minimal/usr/local/bin/ruby \
   poc/safari-stack-test/ruby.wasm

# 3. Serve + open:
python3 poc/safari-stack-test/serve.py 8765
open -a Safari        http://localhost:8765/
open -a "Google Chrome" http://localhost:8765/
```

Click **Run** for each (wasm, script, repeats) combination. Results are
POSTed back to `serve.py` and appended to `results.jsonl` (gitignored).

## Files

- `index.html` — UI + WASI shim glue (loads `@bjorn3/browser_wasi_shim`
  from esm.sh; ~50 KB CDN dependency, not vendored).
- `serve.py` — static file server + `/report` POST endpoint that
  records each run as a JSON line. Forces `application/wasm` MIME.
- `recurse.rb` — depth probe (binary-doubling levels up to 1M).
- `fizzbuzz_1m.rb` — copy of `crates/rubyrs/benches/fizzbuzz_1m.rb` so
  the page can fetch it from the same origin.
- `results.jsonl` *(gitignored)* — one JSON line per run.

## Notes

- `*.wasm` is gitignored. The two binaries are large build artifacts /
  third-party downloads; we keep the repo lean and regenerate locally.
- Don't background the browser tab during fizzbuzz runs — both Safari
  and Chrome throttle inactive tabs, which destroys the timing.
- The bundled WASI shim is not a full implementation; it's enough to
  run the rubyrs and ruby.wasm `_start` entrypoints with one preopen
  directory containing the script. Don't reuse for production embed.
