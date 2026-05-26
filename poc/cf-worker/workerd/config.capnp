# workerd standalone config — runs the rubyrs Worker without
# Cloudflare's edge, without an account, and without CPU/memory
# caps. Same V8 + wasm engine as the CF edge runtime, so a
# `workerd serve` smoke is an apples-to-apples comparison
# against `wrangler dev` (which itself wraps workerd via
# Miniflare).
#
# Run from the poc/cf-worker/ directory:
#   ./workerd/bundle.sh         # esbuild → workerd/dist/worker.mjs
#   ./build.sh                  # cargo + wizer → wasm/rubyrs_worker.wasm
#   npx workerd serve workerd/config.capnp
#
# Module-name design: esbuild leaves the two `import` specifiers
# bare (`./memfs.wasm` for workers-wasi's internal littlefs, and
# `../wasm/rubyrs_worker.wasm` for ours, both relative to the
# bundled `worker.mjs`'s nominal location at `src/`). The
# `modules` entries below carry exactly those names so workerd's
# import resolver finds them at instantiation time.

using Workerd = import "/workerd/workerd.capnp";

const config :Workerd.Config = (
  services = [ ( name = "main", worker = .rubyrsWorker ) ],
  sockets  = [ ( name = "http", address = "*:8080", http = (), service = "main" ) ]
);

const rubyrsWorker :Workerd.Worker = (
  modules = [
    ( name = "worker.mjs",
      esModule = embed "./dist/worker.mjs" ),

    # workers-wasi's bundled littlefs implementation. The bundled
    # worker.mjs imports it as `"./memfs.wasm"` — esbuild kept
    # the specifier bare because of `--external:*.wasm`.
    ( name = "./memfs.wasm",
      wasm = embed "../node_modules/@cloudflare/workers-wasi/dist/memfs.wasm" ),

    # rubyrs_worker (wasm32-wasip1, --no-default-features). Built
    # by ../build.sh from crates/rubyrs/src/bin/wasm_worker.rs.
    # `./` prefix matches what `worker.js` imports and is allowed
    # by workerd's directory-breakout sanity check; the embed
    # path is host-side and is allowed to traverse upward to
    # reach the build output.
    ( name = "./rubyrs_worker.wasm",
      wasm = embed "../src/rubyrs_worker.wasm" ),
  ],
  compatibilityDate = "2025-11-01",
);
