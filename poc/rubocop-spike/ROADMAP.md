# rubocop-rs — Roadmap

> Working roadmap (not yet an ADR). Last updated 2026-07-01.
>
> **Thesis:** ship `brew install rubocop-rs` — RuboCop's *actual* Ruby cops + config
> + plugin ecosystem, delivered like `ruff`/`shellcheck`: a single self-contained
> binary, zero Ruby/gem install, instant boot, steady-state competitive with (or
> faster than) CRuby+YJIT.
>
> This is **not** "a faster CRuby." The durable advantage is the *execution &
> distribution model* (instant boot + single binary + low footprint + sandbox), a
> category CRuby+YJIT structurally can't enter. The JIT work matters because it
> removes the only objection — "but runtime will be slow" — so the model has no
> perf asterisk.

---

## Why bet on this now — the hard risks are already retired

| Risk | Status | Evidence (measured 2026-06-30, `--features jit-native`) |
|---|---|---|
| Steady-state would regress vs YJIT | ✅ retired | both rubocop AST-walk forms beat YJIT — `while` **1.11×**, `.each` **1.41×** — output identical; JIT frontier reached for rubocop shapes |
| Boot win is real | ✅ validated e2e | real rubocop (full cop set) via snapshot image **0.17s vs CRuby cold 0.61s = 3.6× faster**, byte-identical (`e0eb87bf`). NB: rubyrs *cold* is 3.4× slower — the win rides entirely on the image. See Snapshot section |
| Single-binary packaging mechanism | ✅ exists | `rubyrs-wasm-embed` / embedder crate |
| Compat grind is feasible | ✅ proven playbook | zeitwerk 520/520, ActiveSupport, Sinatra, Rack already ground through |

What remains is **work, not unknowns** — chiefly the compat long-pole (M1) and the
plugin decision (M4).

### JIT performance across shapes (rubyrs+JIT vs CRuby+YJIT)

Verified on an **isolated** `--features jit-native` build (2026-06-30; see measurement note below):

| Shape | bench | rubyrs+JIT vs YJIT |
|---|---|---|
| rubocop AST walk (`while` form) | `bench_walk.rb` | **1.11× faster** ✅ |
| rubocop AST walk (`.each` form, full cop body) | `bench_walk_blocks.rb` | **1.41× faster** ✅ |
| skeleton (pure walk) | `bench_decomp.rb` | parity ✅ |
| recursion (fib) | `poc/jit-spike/fib.rb` | ~3–5× faster |
| object hot loop | `crates/rubyrs/benches/jit_oo_dispatch.rb` | 2.3× faster |
| iterator family (sum/map/each) | — | 2.3–11× faster |
| recursion + 2× cross-object call (academic probe) | `bench_treesum.rb` | ⏳ **~1.6× behind — proven STRUCTURAL** (`cfe9ef56`) |

**JIT has reached the rubocop-shape frontier.** Both real rubocop traversal forms
(`while` *and* `.each`/`each_child_node`) now fire native and beat YJIT, output
identical (`chk` matches). The lone remaining gap — `bench_treesum` (double
recursion + two cross-object calls per node) — is now proven **structural** (~1.6×
behind, not incrementally closable) **and is not a rubocop-critical shape** (it was
a general north-star probe). So the JIT side of the thesis is **de-risked**;
chasing the last treesum 1.6× has low payoff.

The journey on `bench_walk`: **18.7× behind YJIT (Jun 29 AM, "structural wall")
→ 1.11× ahead**. The wall was never structural — it was `compile()` value-op
coverage (Array index, `is_a?`/class-guard, Hash r-m-w, Symbol, 2-arg `.each`
block params).

---

## Milestone chain → `brew install rubocop-rs`

| # | Milestone | Status | Notes / gating |
|---|---|---|---|
| **M1** | `require "rubocop"` succeeds on rubyrs | 🔴 in progress | The long pole. Vendor dep tree + fix VM/stdlib gaps. |
| **M2** | Lint one real file, output **byte-identical** to CRuby rubocop | ⬜ | Correctness bar. |
| **M3** | RuboCop's own spec suite green on rubyrs | ⬜ | Community trust artifact ("we faithfully run rubocop"). |
| **M4** | Plugin strategy (rubocop-rails/rspec/performance) | ⬜ **decide early** | #1 adoption risk — see below. |
| **M5** | Fuse runtime + rubocop + deps into one binary (embedder) | ⬜ | Mechanism exists; needs size/strip work. |
| **M6** | Distribution: Homebrew formula + GH releases + multi-arch + CI | ⬜ | arm64/x86_64 macOS + linux. |

**Critical path = M1 (compat).** Known-doable grind, but the long pole.

---

## #1 adoption risk — plugins (decide during M1)

Real RuboCop users almost always load `rubocop-rails`, `rubocop-rspec`, or
`rubocop-performance`. A binary that can't load them is a demo, not a product.
Two architectures — pick a direction early because it shapes M5:

- **(a) Bake in a curated plugin set** — simple, but closed.
- **(b) Runtime-load additional Ruby gem plugins** — open, but reintroduces the
  file-tree dependency for plugins and widens the compat surface.

---

## Do this FIRST — the vertical slice

Don't wait for 600 cops to be green. Prove the thesis end-to-end on the thinnest path:

> rubocop loads → lints one small real file → output **byte-identical** to CRuby
> → packaged as a **single binary** via the embedder → demo `./rubocop-rs ./`
> with **instant boot + identical result**.

Once this slice closes, the thesis moves from "believed" to "visible." Everything
after is breadth (more cops correct, plugins, more platforms) on already-retired
risk.

---

## Current compat status (M1 → M2 crossing)

**M2 progress (2026-07-01):** RuboCop runs the **full default cop set** on a real
file under `RUBYRS_JIT_NATIVE=1`, output **byte-identical** (interp == JIT == CRuby):
`f1.rb` → Style/Documentation + Style/FrozenStringLiteralComment + Style/StringLiterals,
3 offenses / 2 autocorrectable. Getting here fixed a **JIT correctness bug** (value-JIT
`@h[k]` bypassed Hash default_proc → `Config#for_cop` nil → InclusiveLanguage crash;
`8e41a045`). ⚠️ **JIT parity is NOT gated in CI** (diff_cruby runs the default non-jit
build) — that's how a wrong-results JIT bug shipped; add a jit-native diff_cruby job.

**Earlier (2026-06-30):** first single cop byte-identical — `Style/StringLiterals` on
`puts "hello"` → `C:1:6 Correctable`, 1 offense autocorrectable. The Psych wall is
cleared; `diff_cruby` 1033 green; ~18 gaps cleared en route (regex stacked-quantifier
rewrite, `Range#bsearch`, `Set#add?`, `Array#bsearch_index`, `block_given?` in a
deferred Proc, `enum_for` kwargs, Psych block-scalars+anchors for `default.yml`'s 607
keys, ToRuby/ScalarScanner, …). **Next:** run more cops / a real file tree.

**Dependencies vendored** in this dir: rubocop 1.88.0, rubocop-ast 1.38.0,
parser 3.3.0.2, ast 2.4.3, racc 1.8.1, parallel, rainbow, regexp_parser,
ruby-progressbar, unicode-display_width, language_server-protocol, lint_roller.

**Walls (updated 2026-07-01) — first cop runs byte-identical:**

| Wall | Kind | Status |
|---|---|---|
| `racc/parser`, `ast` missing; sprintf `%<name>s`; const-alias lookup; splat/`...` unpack | gem/VM | ✅ fixed |
| inline-cache id wrap at >65535 call sites → method cross-wiring | VM bug | ✅ fixed (`58ff9909`, u16→u32 `1756746f`) |
| Psych/YAML streaming (`Psych::TreeBuilder`), regex stacked-quantifier, `Range#bsearch`, `Set#add?`, `block_given?` in deferred Proc, `enum_for` kwargs (~18 total) | stdlib/VM | ✅ fixed |
| `Array#to_set` (from `require "set"`) — blocked load in a **clean checkout** | ~~gap~~ **not a gap** | ✅ resolved: `to_set` already exists in `stdlib_vendor/set.rb`; the binary was just built without `--features stdlib` (set.rb is behind that gate). Canonical build = `--features stdlib,jit-native`. |
| `Naming/InclusiveLanguage` cop_config `nil` — full default cop set | **JIT correctness bug** (not config) | ✅ fixed (`8e41a045`): value-JIT `@h[k]` (`Config#for_cop` = `@for_cop[cop]`) ignored the Hash default_proc on a miss → nil. Full default cop set now runs under `RUBYRS_JIT_NATIVE=1`, output byte-identical (interp == JIT == CRuby). Regression test `tests/diff/jit_hash_ivar_default_proc.rb`. |

---

## JIT status — frontier reached for rubocop shapes; pivot to M1 compat

The JIT side of the thesis is **de-risked**. Both real rubocop traversal forms beat
YJIT and are output-identical:
- ✅ `while`-form walk — 1.11× faster
- ✅ `.each`/`each_child_node`-form walk (full cop body) — 1.41× faster (`a3c5a3e4`:
  2-arg each-rewrite + per-kind block params)
- ✅ skeleton — parity

The one shape still behind — `bench_treesum` (double recursion + 2 cross-object
calls/node) — was narrowed **1.6× → ~1.38× behind** by ADR 0035 (inline ivar reads,
Phases 1–5; getters/obj-call now beat YJIT 1.28×/1.35×). The residual is **slab-bound**,
and ADR 0036's objects-as-pointers rewrite was **PoC-rejected** (slab is only ~5% of
the gap, not worth it). treesum is **not a rubocop-critical shape** — closing it has
low payoff. Treat the JIT campaign on rubocop shapes as **complete for now**.

**→ The bottleneck is M1→M2 compat + snapshot, not JIT.** Don't sink more into treesum.

See `docs/adr/0034-*` (JIT-first) and `docs/adr/0035-*` (inline object access).

---

## Snapshot / instant-boot — the boot thesis, VALIDATED end-to-end (2026-07-01)

Independently reproduced + measured. Real rubocop, **full default cop set**, on
`f1.rb`, output **byte-identical to CRuby** (interp == JIT == image == CRuby):

| Mode | wall (best of 3) | vs CRuby |
|---|---|---|
| CRuby cold (vendored rubocop 1.88) | 0.61s | 1× |
| rubyrs JIT cold | 2.07s | 3.4× **slower** |
| **rubyrs JIT + snapshot image** | **0.17s** | ✅ **3.6× faster** (12× vs rubyrs cold) |

Mechanism: serialize class graph + heap + constants + closures (`RUBYRS_SNAPSHOT_SAVE`),
restore into a fresh VM before the script runs (`RUBYRS_SNAPSHOT_LOAD`), skipping the
~1.5s `require "rubocop"`. **A capability CRuby lacks** — bootsnap caches bytecode but
must still *execute* every require.

Two things had to be true for this (both now done):
- **`Array#to_set` "gap" was a build-flag issue** — `set.rb` (with `to_set`) is behind
  `--features stdlib`; canonical build = `--features stdlib,jit-native`.
- **Snapshot restore made `require` idempotent** (`e0eb87bf`): the image now carries
  `loaded_features` + `loaded_stdlib_stubs`, so a post-restore `require "rubocop"` is a
  no-op (0.000s). Before this, the re-require re-defined every cop and crashed with
  `Cop RuboCop::Cop::Cop could not be dismissed` — the image run produced NO output
  (the earlier "0.06s" was a crash, not a lint).

**Honest caveat that this benchmark exposed:** rubyrs COLD is 3.4× *slower* than CRuby
(rubocop's cost is loading 600 cops — bulk require — where rubyrs loses; the 12×
bare-boot edge does NOT extend to loading a big Ruby codebase). The whole win rides on
the snapshot. So the product MUST ship the image (via the embedder), and steady-state
run (0.46s vs CRuby 0.09s on a single file — 5× slower) still needs work for large scans.

**Remaining:** a committed, self-contained snapshot benchmark + a CI guard asserting
cold-vs-image byte-identical + a speedup floor (harnesses `_rc_*.rb` are still untracked
scratch; fixtures live in `/tmp`).

---

## References

**Build & run**
```bash
# Build to an ISOLATED target dir — the shared target/release/rubyrs gets
# clobbered by parallel non-jit builds, silently dropping the jit-native feature
# (RUBYRS_JIT_NATIVE is then ignored → benches read as a false "regression").
# NOTE: rubocop needs --features stdlib (set/pathname/psych/… live behind it);
# jit-native alone silently drops set.rb → `Array#to_set` undefined at load.
CARGO_TARGET_DIR=/tmp/jitbuild cargo build --release --features stdlib,jit-native -p rubyrs
RUBYRS_JIT_NATIVE=1 /tmp/jitbuild/release/rubyrs <file.rb>   # JIT on
/tmp/jitbuild/release/rubyrs <file.rb>                        # interpreter
# Canary: `fib.rb` JITon should show ~0.03s *user* time. If it's ~0.7s, the binary
# has no jit-native — rebuild isolated before trusting any number.
```

**Benchmarks**
- `poc/rubocop-spike/bench_walk.rb` — ultimate north-star (rubocop AST walk)
- `poc/rubocop-spike/bench_decomp.rb` — skeleton (pure dispatch); set `_mode.txt` to `skel`
- `poc/rubocop-spike/bench_treesum.rb` — recursion + cross-object call (Gap B)
- `poc/rubocop-spike/bench_walk_blocks.rb` — `.each`-block traversal form
- `poc/jit-spike/fib.rb` — recursion
- `crates/rubyrs/benches/jit_oo_dispatch.rb` — object hot-loop north-star

**Key code**
- `crates/rubyrs/src/jit_native.rs:231` — `compile()` method-body compiler (op coverage gate)
- `docs/adr/0034-*` — JIT-first roadmap
- `crates/rubyrs/src/rouge_native.rs` / `kramdown_native.rs` / `json_native.rs` — the native-shim pattern (precedent for any native substrate)

**Measurement discipline**
- Every newly-lowered op needs an `interpreter == JIT == CRuby` diff test incl. its
  deopt shape (template: `crates/rubyrs/tests/diff/jit_objmethod_sum.rb`).
- Re-benchmark the whole JIT family after shared-plumbing edits (a decline-to-generic
  is correctness-invisible).
- Output parity vs CRuby rubocop is the product's trust bar — verify byte-identical.
