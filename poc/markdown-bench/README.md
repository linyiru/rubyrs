# markdown-bench

Cross-language throughput benchmark of the popular Markdown engines,
plus [`rostdown`](../../crates/rostdown) and the
[`kramdown-rostdown`](../kramdown-rostdown) accelerator, to see where a
Rust-backed kramdown drop-in lands in the field.

## Methodology

Every engine renders the **same 37.5 KB corpus** (`corpus/bench.md`, a
CommonMark-safe mix of prose, headings, lists, blockquotes, inline +
fenced code — no tables/footnotes/math, so every engine handles it and
rostdown stays on its native path). Each CLI **self-times** a warmup
pass + a timed loop with a monotonic clock and reports `ns/op` and
`MB/s` — interpreter/JIT startup and file IO are excluded, so it is
apples-to-apples on the core parse+render cost.

**No syntax highlighting**: every engine emits plain `<pre><code>`.
Highlighting is a separate cost (syntect/rouge) that would otherwise
dominate and is not the parser's job. (commonmarker highlights via
syntect by default — explicitly disabled here.)

> Caveat: these engines do **not** produce identical HTML. pulldown,
> comrak, goldmark, blackfriday, marked, markdown-it target CommonMark/GFM;
> kramdown + rostdown target kramdown-flavor. **Smart typography is enabled
> on pulldown too** (`ENABLE_SMART_PUNCTUATION`) so that work is matched —
> verified equivalent output (`---`→—, `"x"`→“x”, `...`→…). The one thing
> rostdown does that pulldown has no feature for is kramdown **heading
> `id` auto-slugs** (pulldown emits an `id` only from explicit `{#id}`
> syntax, never from the heading text), which also makes rostdown's output
> a few % larger. This measures throughput on the same input, not output
> equivalence.

## Results (Apple M2 Max, arm64, Ruby 3.4.1 / Go / rustc 1.95 / node 22)

```
engine             lang/runtime          ns/op       MB/s      out_B
------             ------------          -----       ----      -----
rostdown arena+simd Rust (opt-in)       108000      348.0      46112
pulldown            Rust (smart punct)  110219      340.6      44022
rostdown default    Rust (zero-dep)     134805      278.5      46112
blackfriday         Go                   386907       97.0      45413
comrak              Rust                 431839       86.9      43878
commonmarker        Ruby→Rust            472072       79.5      49596
goldmark            Go                   703315       53.4      43878
markdown-it         JS/V8                913231       41.1      43878
marked              JS/V8               1191501       31.5      44646
kramdown            Ruby             10263025        3.7      46112
```

> The `rostdown default` row is the **zero-dep, no-`unsafe`** build the
> `kramdown-rostdown` gem ships: **~75× faster than kramdown** (the engine it
> drops in for), 2nd in the field, ~3× comrak. The opt-in `arena`+`simd`
> features (an unsafe scoped bump allocator + an aarch64 NEON byteset) lift it
> to ~348 MB/s — pulldown parity.
>
> **History.** An earlier, much smaller rostdown clocked ~435 default / ~578
> turbo — ahead of pulldown. Reaching **100 % byte-identical** acceptance on
> the full Jekyll + Bridgetown corpus added per-construct hot-path work
> (kramdown-faithful HTML/list re-serialization, a recursive list model,
> source-contiguity tracking, more inline-trigger types) that cost throughput
> on this synthetic CommonMark corpus. A profiling pass clawed back the
> clearest regression — the table-trigger scan ran on every block with no
> memchr fast-bail (+20 % default, +23 % turbo) — and the rest is the
> deliberate price of byte-identical-or-decline correctness. pulldown and
> comrak, unchanged, still match their earlier numbers, so the delta is
> rostdown's added work, not the machine.
>
> Absolute MB/s drifts with machine load; the rostdown/pulldown rows are
> isolated, interleaved median-of-7 runs at N=1000 (±2%), the others from
> a full sweep at N=400 (where the same-process scheduling noise depresses
> pulldown to ~255 — hence the isolated head-to-head for those three).
>
> **rostdown was tuned in steps, output byte-for-byte unchanged
> throughout** (golden corpus + the gem's 211-case differential gate
> green at every step):
>
> | step | MB/s | what |
> |---|---:|---|
> | baseline | 103 | original two-pass owned-AST |
> | convert pass | 121 | killed `format!`/`" ".repeat()`, bulk-copy escaping, pre-sized buffer |
> | parse pass | 153 | inline parser accumulates ordinary text in `push_str` runs, not char-by-char |
> | bump arena | 193 | `--features arena` + `ScopedAlloc`: the AST is pointer-bumped, freed wholesale |
> | byte-ize scans | 239 | `decline_block_scan` (20%→5% self-time), line split, setext — byte loops, not `.chars()` |
> | `is_hr` byte scan | 293 | `is_hr` scanned every prose line 3× (once per `-`/`*`/`_`); now one byte pass that bails on the first char |
> | SWAR `memchr3` escape | 329 | `escape_text` finds `&`/`<`/`>` 8 bytes/word (12%→3.8% self-time) |
> | SWAR `memchr` line split | 351 | `split_lines` finds `\n` a word at a time (was the top parse self-time) |
> | trigger lookup table | 371 | inline parser's "skip ordinary text" loop: a `[bool;256]` membership table, not 15 scattered compares |
> | SWAR `memchr` pipe scan | 381 | `decline_block_scan`'s per-line table check (`\|`) finds the byte a word at a time |
> | byte-level trim | 425 | blank checks, `is_hr`, `decline`, heading/list/setext `trim_end` — ASCII fast paths (was ~7% of self-time in Unicode `str::trim*`) |
> | NEON byteset (`simd`) | 439 | inline trigger scan via an aarch64 NEON nibble-lookup, 16 bytes/iter (opt-in `simd`; scalar table otherwise) |
> | zero-copy AST | 508 | `Block`/`Span` borrow `src` via `Cow<&str>` — pristine prose/code/href runs never copied; only typography/escape allocates |
> | flat-node arena | 573 | nodes live in flat sibling-linked index arenas, not nested `Vec`s — per-node allocations gone (2870 → 776/render) |
>
> Those zero-copy + flat-node + byte-scan optimizations took an early
> rostdown to ~435 default / ~578 turbo — ahead of pulldown. The engine has
> since grown to **100 % byte-identical** kramdown coverage (golden 22/121
> plus the full Jekyll + Bridgetown corpus, the gem's differential), and that
> correctness work cost some of the lead back (see *History* above): it is now
> ~278 default / ~348 turbo — 2nd behind pulldown, ~75× kramdown. The build
> path was data-driven throughout, and still is: each samply profile pins the
> next hot spot — a high-level `str` scan or an allocation — and a byte loop, a
> SWAR/memchr fast-bail, a lookup table, or a borrow removes it.

## Reading the field

- **pulldown-cmark (~351 MB/s, smart punctuation on)** — a streaming
  *pull* parser with no AST and minimal allocation (rustdoc, mdBook,
  Zola). Long the throughput leader among Markdown engines, ~3× faster
  than the AST builders. (Without `ENABLE_SMART_PUNCTUATION` it does less
  work and measures ~393; we keep it on for typography parity.)
- **rostdown (~278 MB/s clean → ~348 with `arena`+`simd`)** is the
  **2nd-fastest** here and **~75× kramdown** (the engine it drops in for),
  ~3× comrak, ~5× goldmark — emitting **byte-identical kramdown HTML** with
  smart typography, kramdown heading `id` auto-slugs (pulldown has no
  equivalent), and decline-checking, at **100 %** acceptance on the real
  corpus. It is a **zero-copy, flat-node, byte-scanning** engine: text borrows
  `src` (`Cow`), AST nodes live in flat index arenas (no per-node `Vec`), and
  the hot scans use SWAR/NEON byte search. An earlier, much smaller rostdown
  led the field at ~435/578; the climb to 100 % byte-identical correctness
  traded part of that lead for faithfulness (the deliberate
  byte-identical-or-decline bargain), with arena+simd keeping it at pulldown
  parity.
- **comrak (86) → commonmarker (79)** shows the Rust-in-Ruby tax is only
  **~9%**. A well-built native gem keeps almost all of the Rust speed —
  exactly the shape `kramdown-rostdown` targets.
- **goldmark (53)** — Hugo's default since 0.60. Clean extensible AST,
  CommonMark-compliant; pays for the AST in throughput. **blackfriday
  (98)** is faster but is legacy/unmaintained and spec-noncompliant.
- **marked (32) / markdown-it (40)** — the JS staples; respectable for
  V8, an order of magnitude behind the Rust leaders.
- **kramdown (3.8 MB/s)** — pure Ruby, **~21× slower than commonmarker
  and ~117× slower than rostdown**. That gap is the entire motivation for
  the accelerator: keep kramdown's exact output, get a compiled engine's
  speed, change zero lines of caller code.

## End-to-end Ruby (with highlighting, realistic Jekyll path)

The table above is raw parse. In a real Jekyll/Bridgetown build the
`kramdown-rostdown` gem also routes code blocks through Rouge (shared by
both paths). See `../kramdown-rostdown/bin/bench.rb`: **21× on prose**,
**~1.7× on code-heavy** docs (there Ruby Rouge dominates — the lever
rubyrs' `carmine` Rust highlighter pulls).

## Run it

```sh
./run.sh                 # builds Rust+Go, runs every engine, prints the table
N=800 NR=200 ./run.sh    # more iterations
REPEAT=64 ruby gen_corpus.rb && ./run.sh   # bigger corpus
./bench_rust_clean.sh    # isolated Rust head-to-head: rostdown default
                         # AND arena+simd vs pulldown/comrak, median-of-7
```

`run.sh` measures rostdown's **default** (zero-dep, no-`unsafe`) build.
`bench_rust_clean.sh` builds it twice — default and `--features turbo`
(arena + NEON simd) — and reports the median of 7 isolated runs, the
numbers used for the rostdown-vs-pulldown rows above.

Contenders & versions: pulldown-cmark 0.12, comrak 0.29, goldmark 1.7.8,
blackfriday v2.1.0, marked 15, markdown-it 14, commonmarker 2.8
(comrak), kramdown 2.5.2.
