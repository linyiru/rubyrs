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
> comrak, goldmark, blackfriday, marked, markdown-it target
> CommonMark/GFM; kramdown + rostdown target kramdown-flavor and *also*
> do smart typography + heading auto-ids (extra work, larger output).
> This measures throughput on the same input, not output equivalence.

## Results (macOS arm64, Ruby 3.4.1 / Go / rustc 1.95 / node 22)

```
engine         lang/runtime          ns/op       MB/s      out_B
------         ------------          -----       ----      -----
pulldown       Rust                 100109      375.0      43638
rostdown       Rust                 128126      293.0      46112
blackfriday    Go                   379526       98.9      45413
comrak         Rust                 439119       85.5      43878
commonmarker   Ruby→Rust            465212       80.7      49596
goldmark       Go                   714362       52.6      43878
markdown-it    JS/V8                915041       41.0      43878
marked         JS/V8               1180138       31.8      44646
kramdown       Ruby               10076917        3.7      46112
```

> Absolute MB/s drifts with machine load; rostdown/pulldown here are
> isolated, interleaved 5× runs (±1%), the others from a full sweep.
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
> | byte-ize scans | 239 | `decline_block_scan` (20%→5% self-time), `escape_*`, line split, setext — byte loops, not `.chars()` |
> | `is_hr` byte scan | 293 | `is_hr` scanned every prose line 3× (once per `-`/`*`/`_`); now one byte pass that bails on the first char |
>
> That's **+185%** (2.85×), closing the gap from 3.6× to **1.28×** of
> pulldown. The path was data-driven: an allocation probe (2,879
> `malloc`s/render vs pulldown's 49) plus a ceiling experiment showed
> allocation was only ~30% of the gap — the rest was per-byte/char
> *scanning* done with high-level `str` methods (`.chars()`, `trim`,
> `filter().count()`), re-run several times per line. A samply profile
> pinned the hot spots (`decline_block_scan` + `is_hr`, both scanning
> every line, were ~33% of runtime combined); byte loops with an early
> bail cut them. The last ~1.28× to pulldown is the same theme taken
> further: SIMD scanning (`memchr`) for `escape_text`, and a zero-copy
> borrowed AST so text isn't copied at all — pulldown's two remaining
> structural edges.

## Reading the field

- **pulldown-cmark (324 MB/s)** is in a class of its own — a streaming
  *pull* parser with no AST and minimal allocation. It's what rustdoc,
  mdBook and Zola use. ~3× faster than the AST builders.
- **rostdown (293 MB/s after tuning + arena)** lands a clear #2, ~3.4×
  ahead of comrak and ~5.6× ahead of goldmark, and now within **1.28×**
  of pulldown — while doing *more* (smart quotes/dashes, `id` slugs) and
  emitting kramdown-byte-identical HTML.
  The whole point: kramdown fidelity at compiled-engine speed. The
  residual ~1.5× to pulldown is **compute, not allocation** (proven by
  the ceiling experiment above): rostdown builds an owned `Block`/`Span`
  AST and copies text, whereas pulldown is a zero-copy streaming pull
  parser that builds no tree. Closing it further means a
  borrowed/streaming redesign, not more allocator work.
- **comrak (86) → commonmarker (79)** shows the Rust-in-Ruby tax is only
  **~9%**. A well-built native gem keeps almost all of the Rust speed —
  exactly the shape `kramdown-rostdown` targets.
- **goldmark (53)** — Hugo's default since 0.60. Clean extensible AST,
  CommonMark-compliant; pays for the AST in throughput. **blackfriday
  (98)** is faster but is legacy/unmaintained and spec-noncompliant.
- **marked (32) / markdown-it (40)** — the JS staples; respectable for
  V8, an order of magnitude behind the Rust leaders.
- **kramdown (3.7 MB/s)** — pure Ruby, **21× slower than commonmarker
  and 28× slower than rostdown**. That gap is the entire motivation for
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
```

Contenders & versions: pulldown-cmark 0.12, comrak 0.29, goldmark 1.7.8,
blackfriday v2.1.0, marked 15, markdown-it 14, commonmarker 2.8
(comrak), kramdown 2.5.2.
