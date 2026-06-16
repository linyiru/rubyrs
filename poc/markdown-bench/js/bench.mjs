// Self-timed markdown render benchmark.
// Usage: node bench.mjs <marked|markdown-it> <file> <iters>
// Emits: "<engine>\t<ns_per_op>\t<mb_per_s>\t<out_bytes>" on stdout.
import { readFileSync } from "node:fs";
import { marked } from "marked";
import MarkdownIt from "markdown-it";

const md = new MarkdownIt({ html: true, linkify: true, typographer: false });

const engine = process.argv[2];
const file = process.argv[3];
const iters = parseInt(process.argv[4] || "200", 10);
const src = readFileSync(file, "utf8");

let render;
if (engine === "marked") render = (s) => marked.parse(s);
else if (engine === "markdown-it") render = (s) => md.render(s);
else throw new Error("unknown engine: " + engine);

let outBytes = 0;
for (let i = 0; i < Math.max(3, (iters / 5) | 0); i++) outBytes = render(src).length;

const t0 = process.hrtime.bigint();
let sink = 0;
for (let i = 0; i < iters; i++) sink += render(src).length;
const t1 = process.hrtime.bigint();
if (sink < 0) console.error(sink);

const ns = Number(t1 - t0);
const nsPerOp = ns / iters;
const mbPerS = (src.length * iters) / (ns / 1e9) / 1e6;
console.log(`${engine}\t${nsPerOp.toFixed(0)}\t${mbPerS.toFixed(1)}\t${outBytes}`);
