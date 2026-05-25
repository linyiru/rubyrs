# Scan targets

Curated list of Ruby codebases worth running `rubyrs-gapscan`
against. This is the **plan** for which reports to add to this
directory next; see [`README.md`](README.md) for the regeneration
mechanics and existing reports.

## What this list is not

- Not an exhaustive catalogue — only targets that pay back the
  scan effort with a concrete decision input.
- Not in priority order within each tier — tiers themselves are
  ranked; items within a tier are picked by current question.
- Not a commitment — each entry comes with a "what question this
  answers" rationale. If no current question maps to it, skip it.

## Selection principle

**Picking a target = picking a question.** Each codebase answers
a different one:

| Question | Best targets |
|---|---|
| "Can rubyrs run a web/templating framework?" | Sinatra, Tilt, Jekyll, Liquid |
| "Can rubyrs run modern, disciplined Ruby code?" | dry-struct, dry-validation |
| "What should `rubund` prioritise?" | Gemfile + gemspec corpus |
| "How much of stdlib is reachable?" | `set`, `csv`, `optparse`, `uri`, `pathname` |
| "Where does method_missing / define_method bite first?" | Minitest, RSpec-core (stress only) |

## Tier 1 — done ✅

The original recommended next batch — all three reports are now
checked in. See [README.md](README.md) for the summary table and
the cross-codebase observations they produced.

| Target | Report | What it added vs Jekyll/Liquid |
|---|---|---|
| **Sinatra** | [sinatra.md](sinatra.md) | First scan where `ModuleNode` *isn't* the top blocker — exposed the `BlockArgumentNode`/`BlockParameterNode`/`RestParameterNode` family as the dominant DSL-framework gap. |
| **dry-struct** | [dry-struct.md](dry-struct.md) | "Modern disciplined Ruby" baseline. Smallest gap surface (19 unique missing classes); confirms the AST view is converging across very different codebases. |
| **Rake** | [rake.md](rake.md) | Larger task-DSL scale. Profile closely matches Jekyll's. `ModuleNode` #1 again. |

**Net conclusion:** ROADMAP Near term #4 (`Module` + `include`)
was validated as the highest-leverage single feature; **it has
since landed on master** (commit `df29e56`) and is the single
biggest gap closure gapscan has measured (jekyll −145 Missing
nodes alone). The block-parameter family (`&block`, `*args`,
splat) is the standing runner-up and matters especially for DSL
frameworks.

## Tier 2 — partly done ✅

The Bundler / Tilt / stdlib slice have been scanned. See
[README.md](README.md) for cross-codebase observations.

| Target | Report | What it added |
|---|---|---|
| **Bundler** | [bundler.md](bundler.md) | Largest single scan (225 files, 106k nodes) yet **highest %Supported (85.25%)**. New blocker surfaced: `KeywordHashNode` ×430 — modern kwargs style. Direct prep for rubund. |
| **Tilt** | [tilt.md](tilt.md) | Confirms `BlockParameterNode` is broadly the DSL-host blocker, not Sinatra-specific. |
| **stdlib `set` / `optparse` / `uri`** | [set](stdlib-set.md), [optparse](stdlib-optparse.md), [uri](stdlib-uri.md) | Stdlib slices sit in the same 80.7–84.7% band as user code — no special blockers beyond what frameworks already surface. `AliasMethodNode` newly visible (set ×16). |

Still pending in Tier 2:

| Target | What it answers |
|---|---|
| **rubygems.org top-100 `.gemspec`** (corpus scan) | Mass-statistics on DSL shape; tells `rubund` which gemspec features to support first. Different workflow — scan as a single tree of N small files. |
| **stdlib `csv` / `pathname` / `digest`** (extra packages bundled as gems, not in `ruby/lib`) | Lower-priority — first three stdlib scans already established the pattern. |

## Tier 3 — stress targets (defer)

Worth knowing about; not worth scanning yet. The metaprogramming-
heavy ones aren't a forever-no — see
[ROADMAP "Metaprogramming: known unknowns"](../ROADMAP.md#metaprogramming-known-unknowns)
for the sequence we'd implement and the decision gate.

| Target | Why defer |
|---|---|
| **RSpec-core** | metaprogramming-saturated; almost everything will be Missing for the wrong reason until rubyrs has `method_missing` / `define_method`. |
| **YARD** | Large, parser-heavy. Will mostly surface stdlib gaps not language gaps. |
| **Rails (any single component)** | Bulk, C-backed gems via dependency, heavy metaprogramming. Out of niche. |
| **Sidekiq / Puma / any concurrency lib** | rubyrs explicitly excludes `Thread`/`Fiber`/`Ractor`. |
| **Any gem whose `lib/` contains `*/native/*.so` or .bundle paths** | gapscan only sees `.rb`; C parts need a separate analyzer (future tool, not gapscan v1). |

## How to add a scan

See [`README.md`](README.md). Briefly:

1. Clone target outside the workspace.
2. `cargo run --release -p rubyrs-gapscan -- scan <path> --format md --top 50 > /tmp/body.md`
3. Prepend a reproducibility header (rubyrs SHA + target SHA + date) — match the format used in `jekyll.md`.
4. Commit under `docs/gap-reports/<name>.md` and add a row to README's summary table.
5. If the new scan changes which feature looks highest-ROI, mention it in `docs/ROADMAP.md`'s "Real-world gap data" section.
