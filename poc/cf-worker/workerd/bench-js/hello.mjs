// Minimal JS-only Worker — baselines pure workerd process
// startup + V8 isolate spawn + fetch-handler entry cost. No
// wasm, no WASI shim, no rubyrs. Difference vs the rubyrs
// worker's first-request wall-time attributes the gap to
// wasm parse/compile + Runtime construction.
export default {
  async fetch() {
    return new Response("hi\n", { status: 200 });
  },
};
