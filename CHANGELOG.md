# Changelog

All notable changes to rubyrs will be recorded here. The format is loosely
based on [Keep a Changelog](https://keepachangelog.com/), and this project
follows [Semantic Versioning](https://semver.org/) once we hit 0.1.

## [Unreleased]

### Internal
- **Error / Span infrastructure** (P0-B-1): new `src/error.rs` with
  `Span` (byte offset into source), `RubyError` (closed set covering
  SyntaxError / NoMethodError / ArgumentError / TypeError / RuntimeError
  / NameError), `TrapFrame`, and `Trap` (error + backtrace). `Expr`
  is now wrapped as `Spanned<Expr>` (alias `SExpr`); `Proto` gets a
  parallel `op_spans: Vec<Span>` and a `filename: Rc<str>`. The
  panic→Trap migration itself is the next commit; this one just
  wires the plumbing so spans flow from Prism → Expr → Op without
  changing observable behaviour.
- **Module split** (P1-A): `src/main.rs` (1600 lines) split into
  `ast.rs`, `value.rs`, `heap.rs`, `bytecode.rs`, `compiler.rs`,
  `vm.rs`, plus a 55-line CLI `main.rs`. Move-only refactor:
  fixtures emit stdout bit-identical to the pre-split binary. Sets
  up the seam for the upcoming `lib.rs` / embedding API (P1-C).
- `Op` and `BinOpKind` now derive `Copy` (P0-C). The dispatch loop's
  `code[ip].clone()` becomes a plain `Copy`. The previous `clone()` was
  already optimised away by LLVM (all payloads were already POD), so
  this is a structural correctness change rather than a measurable
  speedup. Future Op variants must remain POD or carry a `SymId` /
  similar index instead of an `Rc<str>`.

### Fixed
- **GC root hole in native-driven iterators** (P0-A). `Array#map`,
  `Array#each`, and `Hash#each` accumulated state in Rust-local `Vec`s
  that weren't visible to the mark phase; a sufficiently large `map`
  could read use-after-free objects. Now uses an explicit `Vm.pinned`
  root list. `STRESS_GC=1 cargo test` exercises this in CI.

### Added
- `STRESS_GC=1` env flag forces a full collection on every potential
  GC point. Wired into CI as a second job.
- ADR 0005: pinned stack for native-driven loops.
- Symbol literal (`:foo`) and shorthand hash key syntax (`{name: "x"}`)
- String interpolation: `"hello #{name}"`, mixed with method calls and math
- `Nil#to_s` / `inspect` / `nil?`, `Bool#to_s` to round out built-ins
  needed by interpolation
- GitHub Actions CI: Linux + macOS, build + test on every push and PR
- LICENSE files: dual MIT OR Apache-2.0
- Crate metadata in `Cargo.toml` (description, license, repository)
- `docs/` directory with structured project documentation
- Architecture Decision Records (`docs/adr/`)
- `CHANGELOG.md` and `CONTRIBUTING.md`

### Internal
- Specialised `Op::BinOp(BinOpKind)` for `+ - * / % == != < <= > >=` —
  Int+Int fast path avoids generic method dispatch
- 1M-fizzbuzz: 0.67 s → 0.44 s (2.3× of CRuby's interpreter)

## [0.0.x — development]

Initial PoC and milestones leading up to this point. All work pre-tag is
in the commit log; the changelog is canonical from here forward.
