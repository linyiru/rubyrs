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

## Tier 1 — recommended next batch

These give the highest signal-per-scan-minute. All pure Ruby, all
DSL-shaped or framework-shaped (rubyrs's stated niche), all
moderate size.

| Target | Repo | Size (lib/) | Why it pays back |
|---|---|---:|---|
| **Sinatra** | sinatra/sinatra | ~3K LoC | Web DSL — completely different shape from Jekyll. Heavy `define_method` for routes; exposes the first wall when rubyrs lacks dynamic dispatch. |
| **dry-struct** | dry-rb/dry-struct | ~1-2K LoC | Modern, *disciplined* Ruby: `attr_*`, `include`, keyword args, almost no metaprogramming. Tightest baseline for "can rubyrs run a contemporary library at all". |
| **Rake** | ruby/rake | ~5-8K LoC | Task DSL, near-relative of Brewfile/Gemfile but with namespacing, dependencies, `instance_eval` blocks. Validates the DSL niche at the next size up. |

After Tier 1 we expect a clear answer to: *which one missing Prism
node (or built-in method) would close the most gap across multiple
real codebases at once*. That's the input for the next feature PR.

## Tier 2 — specific-gap probes

Run these only when investigating a specific question.

| Target | What it answers |
|---|---|
| **Bundler (Gemfile + gemspec evaluation paths only)** | Direct prep for `rubund`; both files are pure DSLs. |
| **Tilt** | Template-multiplexer pattern; lots of dynamic class registration. |
| **stdlib slice**: `set`, `csv`, `optparse`, `uri`, `pathname`, `digest` | "What fraction of stdlib could rubyrs ship native?" Each is small enough to scan individually; aggregating builds a stdlib-coverage curve. |
| **rubygems.org top-100 `.gemspec`** (corpus scan) | Mass-statistics on DSL shape; tells `rubund` which gemspec features to support first. Different workflow — scan as a single tree of N small files. |

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
