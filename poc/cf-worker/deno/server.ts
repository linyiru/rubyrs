// Deno self-host of rubyrs.wasm — third deployment target after
// CF Workers (managed) and workerd (self-host). Demonstrates that
// the same `wasm32-wasip1` artifact runs unchanged on a different
// V8 host as long as it provides a Preview 1 WASI shim.
//
// Why this exists: the broader thesis of the PoC is "one
// rubyrs.wasm bytes, run on any wasm-host edge runtime, no vendor
// lock-in". Deno is the reference example on the JS-runtime side —
// Deno Deploy : Deno  ::  CF Workers : workerd (managed/self-host
// duality with the same engine on both ends).
//
// Why NOT @cloudflare/workers-wasi here: it bundles `memfs.wasm`
// inside the npm package and imports it via `import wasm from
// "./memfs.wasm"`. Deno's module loader eagerly walks the wasm
// module's import section and treats `wasi_snapshot_preview1` as
// an unresolvable JS package (it's actually a wasm import to be
// supplied at instantiate time). That works under workerd's
// loader but not Deno's; the symptom is a startup ModuleNotFound
// error before the server ever listens.
//
// Why NOT jsr:@std/wasi: that module was deprecated (Oct 2023) and
// removed (Nov 2023) from Deno's std library before the JSR cut-
// over; it never shipped to JSR. The URL 404s today.
//
// We use `@bjorn3/browser_wasi_shim` — pure-JS Preview 1 shim with
// no internal wasm dep, designed for buffer-shaped stdin/stdout
// via `File`/`ConsoleStdout` fds. Same logical role as workers-wasi
// in worker.js; the differences live in the small wiring section
// below.
//
// Run:
//   ./build.sh                              # produce src/rubyrs_worker.wasm
//   deno run --allow-net --allow-read deno/server.ts
//   curl -X POST --data-binary 'puts 1+1' http://localhost:8000

import {
  ConsoleStdout,
  File,
  OpenFile,
  WASI,
} from "@bjorn3/browser_wasi_shim";

// Load + compile the module once at boot. Deno's V8 caches the
// compiled `WebAssembly.Module` for the lifetime of the process
// (same shape workerd uses), so per-request cost is the
// `instantiate` + run, not the parse.
const wasmBytes = await Deno.readFile(
  new URL("../src/rubyrs_worker.wasm", import.meta.url),
);
const wasmModule = await WebAssembly.compile(wasmBytes);

// Per-isolate hit counter (parallels worker.js). A Deno process
// has one long-lived isolate, so this is effectively a request
// counter — but the header name stays consistent with the CF /
// workerd surface so the cold/warm bucketing harness works
// unchanged across all three deployment targets.
let invocations = 0;

Deno.serve({ port: 8000 }, async (request) => {
  invocations += 1;
  const invocation = invocations;

  if (request.method !== "POST") {
    return new Response(
      "POST Ruby source as the request body (text/plain).\n",
      { status: 405, headers: { "content-type": "text/plain" } },
    );
  }

  // Buffer the request body. browser_wasi_shim's `File` takes a
  // Uint8Array; it does not accept a ReadableStream. For our PoC
  // (small Ruby source per request) buffering is fine; for large
  // / streaming inputs we'd need a custom Fd subclass that
  // implements `fd_read` against the live stream.
  const bodyBytes = new Uint8Array(await request.arrayBuffer());

  // Capture stdout / stderr via ConsoleStdout's callback hook.
  // Same buffer-then-respond pattern as worker.js — we need the
  // exit code before deciding the HTTP status.
  const stdoutChunks: Uint8Array[] = [];
  const stderrChunks: Uint8Array[] = [];

  // fds array maps to WASI file descriptors in order: 0=stdin,
  // 1=stdout, 2=stderr, then anything else (preopens, etc.). We
  // wrap stdin as a read-only OpenFile over the request body so
  // rubyrs's `io::stdin().read_to_string(...)` pulls the Ruby
  // source out byte-for-byte.
  const wasi = new WASI(
    ["rubyrs_worker"],   // argv
    [],                  // env
    [
      new OpenFile(new File(bodyBytes)),                 // fd 0: stdin
      ConsoleStdout.lineBuffered((line: string) => {     // fd 1: stdout
        stdoutChunks.push(new TextEncoder().encode(line + "\n"));
      }),
      ConsoleStdout.lineBuffered((line: string) => {     // fd 2: stderr
        stderrChunks.push(new TextEncoder().encode(line + "\n"));
      }),
    ],
  );

  // browser_wasi_shim exposes the import object as `wasiImport`,
  // same name as workers-wasi.
  const instance = await WebAssembly.instantiate(wasmModule, {
    wasi_snapshot_preview1: wasi.wasiImport,
  }) as WebAssembly.Instance;

  // command-shape wasm exports `_start`. browser_wasi_shim's
  // `start()` throws `WASIProcExit` on `proc_exit`; catch it so a
  // Ruby trap (exit 1) doesn't crash the server. CF / Node also
  // route exits through a thrown sentinel (different class names),
  // but we don't need the host's class — just `code`-shaped exit.
  let exitCode = 0;
  try {
    wasi.start(instance);
  } catch (e) {
    if (e && typeof e === "object" && "code" in e) {
      exitCode = (e as { code: number }).code;
    } else {
      throw e;
    }
  }

  const decoder = new TextDecoder();
  const stdout = stdoutChunks.map((c) => decoder.decode(c)).join("");
  const stderr = stderrChunks.map((c) => decoder.decode(c)).join("");

  const headers = {
    "content-type": "text/plain",
    "x-rubyrs-invocation": String(invocation),
  };
  if (exitCode !== 0) {
    return new Response(
      `rubyrs exited ${exitCode}\n\n${stderr || stdout}`,
      { status: 500, headers },
    );
  }
  return new Response(stdout, { status: 200, headers });
});
