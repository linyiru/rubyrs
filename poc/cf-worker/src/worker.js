// PoC Worker: pipe HTTP request body → wasm stdin → HTTP response.
//
// Shape:
//   POST /  with `text/plain` body containing Ruby source
//   → 200  with the Ruby script's stdout as the response body
//   → 500  with the trap message on Ruby-side error
//
// V8 caches the compiled `WebAssembly.Module` across requests in
// the same isolate (Cloudflare confirms this in the runtime-apis
// docs), so the per-request cost is `WebAssembly.instantiate` +
// stdio plumbing, not parse/compile. The first request in a cold
// isolate pays the parse cost — that's the only number that has
// to be measured on the real edge (Miniflare/`wrangler dev` does
// not faithfully simulate isolate cold-starts).
//
// References:
//   - @cloudflare/workers-wasi: https://github.com/cloudflare/workers-wasi
//   - WASM module bindings:    https://developers.cloudflare.com/workers/runtime-apis/webassembly/
import { WASI } from "@cloudflare/workers-wasi";
import wasmModule from "../wasm/rubyrs_worker.wasm";

export default {
  async fetch(request) {
    if (request.method !== "POST") {
      return new Response(
        "POST Ruby source as the request body (text/plain).",
        { status: 405, headers: { "content-type": "text/plain" } },
      );
    }

    // Capture stdout / stderr via a WritableStream sink. We can't
    // hand the Worker's outgoing Response stream directly to wasi
    // here because we need to await wasi.start() to know whether
    // the script trapped before deciding on the status code; the
    // straightforward shape is "buffer, then respond". For
    // streaming responses, swap to a TransformStream and pipe its
    // readable side into the Response — that's a follow-up once
    // the round-trip works.
    const stdoutChunks = [];
    const stderrChunks = [];
    const collect = (sink) => new WritableStream({
      write(chunk) { sink.push(chunk); },
    });

    const wasi = new WASI({
      args: ["rubyrs_worker"],
      env: {},
      stdin: request.body,            // pipe request body straight in
      stdout: collect(stdoutChunks),
      stderr: collect(stderrChunks),
      returnOnExit: true,             // don't throw ProcessExit
    });

    const instance = await WebAssembly.instantiate(wasmModule, {
      wasi_snapshot_preview1: wasi.wasiImport,
    });

    const exitCode = await wasi.start(instance);

    const decoder = new TextDecoder();
    const stdout = stdoutChunks.map((c) => decoder.decode(c)).join("");
    const stderr = stderrChunks.map((c) => decoder.decode(c)).join("");

    if (exitCode && exitCode !== 0) {
      return new Response(
        `rubyrs exited ${exitCode}\n\n${stderr || stdout}`,
        { status: 500, headers: { "content-type": "text/plain" } },
      );
    }
    return new Response(stdout, {
      status: 200,
      headers: { "content-type": "text/plain" },
    });
  },
};
