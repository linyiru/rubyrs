# rostdown real-content acceptance

A repeatable measurement of how much **real-world** Markdown rostdown
renders natively versus declines (→ Ruby-kramdown fallback). It answers
the practical question for the `kramdown-rostdown` gem: *on real Jekyll /
Bridgetown content, what fraction of pages get the Rust fast path?*

This complements the throughput benchmark in `../markdown-bench` (how fast
the accepted path is) and the correctness gates in the rostdown crate
(`tests/golden.rs` + the kramdown differentials, which gate that accepted
output is byte-identical).

## Sources (fetched, not vendored)

Nothing is checked in. `fetch.sh` clones permissively-licensed sources on
demand into `corpus/` and writes front-matter-stripped bodies to
`prepared/`; both are git-ignored, so there is no redistribution to
license.

| source | repo | license |
| --- | --- | --- |
| jekyll | github.com/jekyll/jekyll (`docs/_posts`, `_docs`, `_tutorials`) | MIT |
| bridgetown | github.com/bridgetownrb/bridgetown (`bridgetown-website/src/_posts`, `_docs`) | MIT |

Both are the gem's actual target ecosystems. To add a source, append a
`name|git-url|content-roots|license` line in `fetch.sh` (keep it
permissive — MIT / BSD / Apache / CC0 / CC-BY; avoid CC-BY-SA, whose
share-alike would travel).

## Usage

```sh
./fetch.sh   # clone + prepare (idempotent; rm -rf corpus/<name> to refresh)
./run.sh     # per-source + combined accept-vs-decline + decline histogram (Rust-only)
./verify.sh  # the byte-identity GATE: accepted ⇒ bytes equal kramdown (needs Ruby + kramdown)
```

`run.sh` answers *"what fraction did rostdown render rather than decline?"*
(fast, no Ruby). `verify.sh` answers the stricter, real question —
*"…and was every rendered page byte-identical to kramdown?"* — by diffing
each accepted page against `kramdown_oracle.rb` (the gem profile, syntax
highlighting off to match rostdown's NoHighlight). **`WRONG` must be 0**:
a nonzero count is an accept-but-wrong bug, which is worse than a decline
(a decline still renders correctly via the Ruby fallback). Treat
`verify.sh`'s acceptance number, not `run.sh`'s, as the headline.

## Caveats

- **Front matter stripped** the way an SSG does before Markdown (so a
  leading `---…---` block isn't mis-parsed). Liquid is **not** expanded —
  `{% … %}` / `{{ … }}` survive as literal text, which both rostdown and
  kramdown treat literally, so it rarely changes the accept/decline
  outcome but isn't a perfect mirror of a real build.
- `decline_scan` reports the **first** decline reason per file, so the
  histogram under-counts secondary reasons.
- In `run.sh`, "accepted" means only that rostdown rendered rather than
  declined — it does **not** prove the output is correct. `verify.sh` adds
  the byte-identity check that does; prefer its number.

## Findings (2026-06-17, byte-identity verified)

`verify.sh` — **0 accept-but-wrong**:

| source | byte-identical acceptance |
| --- | ---: |
| bridgetown | 60.5 % (78/129) |
| jekyll | 54.5 % (110/202) |
| **combined** | **56.8 % (188/331)** |

Top decline reasons (combined, `run.sh`): `html-block` (49), `table` (20),
`ald-ial-extension` (19), `inline-html-or-autolink` (16),
`opt-space-block` (7).

**The accept-but-wrong correction.** Earlier snapshots (40.2 % → 50.5 % →
a raw 57.4 %) counted accept-vs-decline only — they were never byte-checked
against kramdown. The first byte-identity sweep found **12 accepted pages
whose HTML differed from kramdown** (the cardinal sin: silently wrong, not
a safe fallback). All 12 were fixed — by declining the constructs we can't
reproduce (Liquid-in-link-defs, chained/blockquote/header IALs, cross-line
emphasis & links, opt-space lazy-continuation whitespace) and by correctly
supporting one (length-aware fence close). That moved those 12 from
accept→decline, so the honest post-correction figure was **53.8 %, with
WRONG = 0** — lower than the inflated 57.4 %, but every accepted page is now
provably byte-identical. Two follow-on features — HTML entity resolution
(kramdown `:as_char`) and spaced link-definition destinations (so Liquid
`{{ … }}` URLs resolve) — then lifted it to the current **56.8 %**, still
WRONG = 0. `verify.sh` is the standing gate that keeps it that way.

**Reading it.** Acceptance is content-dependent: crafted CommonMark-safe
prose (`../markdown-bench/corpus/bench.md`) is 100 %; real Jekyll/Bridgetown
content ~57 %. The dominant remaining blockers are *core kramdown features
the engine does not yet do* — raw HTML blocks and IAL/ALD (`{:.class}`,
`{:toc}`). Every declined page still renders correctly via the
Ruby-kramdown fallback; declining only forgoes the speed-up. So the
highest-leverage work for broader real-world acceleration is raw-HTML
blocks and IAL/ALD.
