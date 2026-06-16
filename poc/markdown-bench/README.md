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
rostdown       Rust                  98533      381.0      46112
pulldown       Rust                 100109      375.0      43638
blackfriday    Go                   392369       95.7      45413
comrak         Rust                 436619       86.0      43878
commonmarker   Ruby→Rust            471132       79.7      49596
goldmark       Go                   720665       52.1      43878
markdown-it    JS/V8                912797       41.1      43878
marked         JS/V8               1196911       31.4      44646
kramdown       Ruby               10025292        3.7      46112
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
> | byte-ize scans | 239 | `decline_block_scan` (20%→5% self-time), line split, setext — byte loops, not `.chars()` |
> | `is_hr` byte scan | 293 | `is_hr` scanned every prose line 3× (once per `-`/`*`/`_`); now one byte pass that bails on the first char |
> | SWAR `memchr3` escape | 329 | `escape_text` finds `&`/`<`/`>` 8 bytes/word (12%→3.8% self-time) |
> | SWAR `memchr` line split | 351 | `split_lines` finds `\n` a word at a time (was the top parse self-time) |
> | trigger lookup table | 371 | inline parser's "skip ordinary text" loop: a `[bool;256]` membership table, not 15 scattered compares |
> | SWAR `memchr` pipe scan | 381 | `decline_block_scan`'s per-line table check (`\|`) finds the byte a word at a time |
>
> That's **+270%** (3.7×) — from 3.6× *behind* pulldown to **ahead**
> (~381 vs ~375 MB/s, rostdown faster in 8/8 interleaved runs), while
> rostdown does strictly *more*
> (smart typography, heading `id` slugs, decline-checking, and
> byte-identical kramdown output). The path was data-driven: an
> allocation probe (2,879 `malloc`s/render vs pulldown's 49) plus a
> ceiling experiment showed allocation was only ~30% of the gap — the
> rest was per-byte/char *scanning* with high-level `str` methods
> (`.chars()`, `trim`, `filter().count()`), re-run several times per
> line. Each samply profile pinned the next hot scan; a byte loop, a
> dependency-free SWAR `memchr`, or a lookup table cut it. What remains
> toward *beating* pulldown is the same theme once more — a SIMD byteset
> for the inline trigger scan — plus a zero-copy
> borrowed AST so text isn't copied at all — pulldown's two remaining
> structural edges.

## Reading the field

- **pulldown-cmark (324 MB/s)** is in a class of its own — a streaming
  *pull* parser with no AST and minimal allocation. It's what rustdoc,
  mdBook and Zola use. ~3× faster than the AST builders.
- **rostdown (381 MB/s after tuning + arena)** is now the **fastest in
  the field** — past pulldown (8/8 interleaved runs), ~4.4× comrak,
  ~7.3× goldmark — *while doing strictly more* (smart quotes/dashes, `id`
  slugs, decline-checking, byte-identical kramdown output). It reached
  this by pairing the `arena` (which makes its owned AST nearly
  allocation-free) with byte-level/SWAR scanning that out-runs pulldown's
  per-construct work on this prose-heavy corpus. The remaining
  architectural lever (allocation: the arena path is still ~+55% over the
  system allocator) is what a zero-copy borrowed AST would bank *without*
  the arena — and would widen the lead further.
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
