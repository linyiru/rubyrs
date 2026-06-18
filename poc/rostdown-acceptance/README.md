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
| bridgetown | 94.6 % (122/129) |
| jekyll | 97.0 % (196/202) |
| **combined** | **96.1 % (318/331)** |

Top decline reasons (combined, `run.sh`) are now a flat tail of kramdown
quirks: multi-block / multi-line-table list items, cross-line emphasis
pairing in lazy list continuations, list-trailing-whitespace hard breaks,
indented code that is really a lazy paragraph continuation inside a Liquid
block, `{:toc}`/ALD extensions, doctype/comment HTML, and multi-line span
HTML blocks. The reproducible block/inline tails are all rendered now:
strikethrough, hard breaks, nested link text, `{#id}`, leading IALs,
OPT_SPACE-list paragraph interruption, indented code, OPT_SPACE fences, the
`<table>` family + custom/unknown HTML elements, span-content `code`/`samp`
elements, autolinks (`<https://…>`/`<user@host>`), pipe-tables inside a
single-line list item, and a span element opening a paragraph.

**On 100 %.** Correctness is already 100 % — every accepted page is
byte-identical and every declined page renders correctly via the Ruby
fallback (`WRONG = 0`). The acceleration ratio is what climbs. The
remaining decliners split into *reproducible* tails (hard breaks,
strikethrough, indented code, link-text edge cases, OPT_SPACE fences) and
*impractical* ones whose byte-identity would mean reproducing kramdown
quirks at real accept-but-wrong risk — raw-text/custom HTML elements
(`<table>`, `<script>`, `<sl-button>`), tables kramdown builds inside list
items/blockquotes from a stray pipe, `{:toc}` generation, multi-block list
items. Literal 100 % acceleration is the asymptote the
byte-identical-or-decline design deliberately trades away for safety; the
practical ceiling is ~85-90 %.

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
provably byte-identical. Follow-on features then lifted it, all still WRONG = 0: HTML entity
resolution (kramdown `:as_char`), spaced link-definition destinations (so
Liquid `{{ … }}` URLs resolve), the blockquote "note box" IAL
(`> …\n{:.note}` → `<blockquote class="note">`), and — the biggest lever —
a kramdown-faithful re-serialization of common raw HTML blocks
(`<div class="note">…`, `<figure>`, nested block/void elements), the
pipe/table boundary (a block with a pipe-less line is a paragraph, not a
decline), leading block IALs (`{:.note}\ntext` → `<p class="note">`), and
inline HTML elements (void like `<br>` plus markdown-content ones like
`<a>`/`<abbr>`/`<sub>`), the `{#id}` header-id shorthand, hard line breaks
(`<br />`), nested brackets / linked images in link text, GFM
strikethrough (`~~x~~`), OPT_SPACE list paragraph interruption, and indented
code blocks — then OPT_SPACE fenced code, the `<table>` family + custom/
unknown HTML elements re-serialized like a `<div>`, span-content
`code`/`kbd`/`samp`/`var` (well-formed nested tags kept), autolinks, a
pipe-table built inside a single-line list item, a span element opening a
paragraph, single-line list-item trailing whitespace, kramdown's
single-token fenced-code info rule, the biggest lever — a full recursive
list-item parser (multi-block / loose / nested / lazy-continuation /
multi-row-pipe-table items, with kramdown's tight/loose rules) — then block
IALs on fenced code and tables (both sides), a `|` inside an inline HTML
tag no longer mistaken for a table separator, depth-matched link
destinations (`[t](…Fork_(x))`, `[t]((u))`, kramdown's `LINK_PAREN_STOP`),
smart quotes directly after a code span (`` `x`'s `` → ’s), and an
opposite-kind list marker indented shallow under an ordered item
(`1. a:\n  * b` → nested `<ul>`), and a base-aware list parser that parses an
OPT_SPACE list in place so a lazy continuation keeps its residual leading
space verbatim — then a uniformly-loose list whose multi-block items are
blank-separated (`[Para, Blank, Code/List]`): kramdown only mixes per-item
tight/loose when a block DIRECTLY ABUTS the leading paragraph, so a blank-
separated one stays loose and renders natively — then full PER-ITEM tight/loose
mixing: kramdown makes an item's leading paragraph "transparent" (inline, no
`<p>`) when a block abuts it AND a cross-item condition holds, so a loose list
can mix tight and loose items (and the transparency carries to a trailing
item); reproducing that fold renders the last list-mixing files natively.
Current: **96.1 %**. `verify.sh` is the standing gate that keeps it WRONG = 0.

**Reading it.** Acceptance is content-dependent: crafted CommonMark-safe
prose (`../markdown-bench/corpus/bench.md`) is 100 %; real Jekyll/Bridgetown
content ~96 %. The remaining blockers are the impractical tail — constructs
whose byte-identity would mean reproducing kramdown quirks at real
accept-but-wrong risk, confirmed by probing each against the oracle:
a blank-separated "lonely" IAL whose
attachment target is context-dependent, `{:toc}`/ALD generation, indented
code that is really a lazy paragraph continuation inside a Liquid block, a
tilde run that is really `~~`-strikethrough with a literal remainder
(`~~~x~~~` → `~<del>x</del>~`), and `<script>`/comment/doctype/multi-line-span
HTML. Every declined page still
renders correctly via the Ruby-kramdown fallback; declining only forgoes the
speed-up.
