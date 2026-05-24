# Changelog

All notable changes to rubyrs will be recorded here. The format is loosely
based on [Keep a Changelog](https://keepachangelog.com/), and this project
follows [Semantic Versioning](https://semver.org/) once we hit 0.1.

## [Unreleased]

### Added
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
