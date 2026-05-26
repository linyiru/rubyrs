# rubyrs on Cloudflare Workers — PoC

Goal: prove rubyrs.wasm runs on Cloudflare Workers (V8 isolate +
WASI Preview 1 polyfill), end-to-end, locally via `wrangler dev`.

## Shape

```
HTTP POST  body=Ruby source
       ↓
   Worker fetch handler (src/worker.js)
       ↓ pipes body as stdin
   @cloudflare/workers-wasi  (WASI preview1 shim in JS)
       ↓
   rubyrs_worker.wasm  (wasm32-wasip1 bin reading stdin via Runtime::eval)
       ↓ captures stdout
HTTP 200  body=Ruby script output
```

The worker bin (`crates/rubyrs/src/bin/wasm_worker.rs`) reads
stdin → `Runtime::eval` → stdout. The Worker pipes
`request.body` straight in; it does not touch the in-isolate
filesystem (workers-wasi's littlefs has no public pre-population
API, see [research notes](#research-notes)).

## Prerequisites

- Rust toolchain matching `rust-toolchain.toml`
- `rustup target add wasm32-wasip1`
- `WASI_SDK_PATH` pointing at a wasi-sdk install (same as
  `tests/wasm/smoke.sh` — needed for the wasi_stub.c compile in
  build.rs). Download from
  https://github.com/WebAssembly/wasi-sdk/releases.
- `node` + `npm`
- `wizer` is optional; included in the build path when present
  (`cargo install wizer-cli`).

## Quick start

```sh
# From this directory.
npm install            # @cloudflare/workers-wasi + wrangler
./build.sh             # cargo → (optional) wizer → wasm/rubyrs_worker.wasm
npx wrangler dev       # local V8 (workerd) on http://localhost:8787

# In another terminal:
curl -X POST --data-binary 'puts (1..5).sum' http://localhost:8787
# → 15
```

## Layout

```
poc/cf-worker/
├── wrangler.toml          # Worker config + CompiledWasm rule
├── package.json           # workers-wasi + wrangler
├── build.sh               # cargo build → wizer → copy artifact
├── src/worker.js          # fetch handler
├── wasm/                  # build.sh writes rubyrs_worker.wasm here
└── README.md
```

## Knobs / next steps

- **Streaming response**: replace the buffered stdout capture in
  `worker.js` with a `TransformStream` whose readable side is
  the `Response` body. Lets long-running Ruby see incremental
  output.
- **CPU / memory caps**: surface `RUBYRS_DEADLINE_MS` etc. via
  WASI `env`. Worker fetch handler can set a deadline below the
  Worker's own 30 s CPU cap so traps come from rubyrs with
  context rather than from the edge with `Error 1102`.
- **Wizer cold-start measurement**: only meaningful on the real
  edge — Miniflare/`wrangler dev` does not reproduce isolate
  cold-start. Deploy + `wrangler tail` to measure.
- **Static-script mode**: for a fixed-DSL deployment, replace
  `request.body` with an embedded `include_str!`'d script and
  pin the wasm at build time. Removes the per-request stdin
  plumbing and lets the response be streamed.

## Research notes

- `@cloudflare/workers-wasi` does not expose a way to write into
  the FS before instantiation; `preopens` is a `string[]` of
  names only. Stdin is the documented input channel for
  command-shape wasm — hence the bin reads from stdin.
- Local dev (`wrangler dev`) uses Miniflare v3 → `workerd`, the
  same runtime as production. Module loading and the
  `wasi_snapshot_preview1` shim behave identically. Cold-start
  timing and the 10 ms / 30 s CPU caps are **not** enforced
  locally — only on the deployed edge.
- `_start` is the entry; workers-wasi's `wasi.start(instance)`
  drives it. Re-instantiating per request is the documented
  pattern; V8 caches the compiled `WebAssembly.Module`.
