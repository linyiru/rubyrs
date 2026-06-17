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
./run.sh     # per-source + combined acceptance and the decline histogram
```

## Caveats

- **Front matter stripped** the way an SSG does before Markdown (so a
  leading `---…---` block isn't mis-parsed). Liquid is **not** expanded —
  `{% … %}` / `{{ … }}` survive as literal text, which both rostdown and
  kramdown treat literally, so it rarely changes the accept/decline
  outcome but isn't a perfect mirror of a real build.
- `decline_scan` reports the **first** decline reason per file, so the
  histogram under-counts secondary reasons.
- "Accepted" means rostdown rendered rather than declined. That the
  accepted output is byte-identical to kramdown is gated separately (the
  rostdown crate's golden corpus + differentials), not re-checked here.

## Findings (2026-06-16)

| source | acceptance |
| --- | ---: |
| bridgetown | 45.7 % (59/129) |
| jekyll | 36.6 % (74/202) |
| **combined** | **40.2 % (133/331)** |

Top decline reasons (combined): `html-block` (41), `ald-ial-extension`
(31), `opt-space-block` (26), `link-definition` (22), `table` (20),
`inline-html-or-autolink` (13), `entity` (10), `image` (6).

**Reading it.** Acceptance is content-dependent: crafted CommonMark-safe
prose (`../markdown-bench/corpus/bench.md`) is 100 %; this repo's own
technical docs ~59 %; real Jekyll/Bridgetown content ~40 %. The dominant
real-world blockers are *core kramdown features* — raw HTML blocks,
IAL/ALD (`{:.class}`, `{:toc}`), reference-style links, entities — not the
inline-link / table / list / smart-quote work that lifted this repo's own
docs. Every declined page still renders correctly via the Ruby-kramdown
fallback; declining only forgoes the speed-up. So the highest-leverage
work for broad real-world acceleration is IAL/ALD and raw-HTML blocks.
