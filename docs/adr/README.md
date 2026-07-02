# Architecture Decision Records (ADRs)

An ADR is a short document that captures **one architectural decision**
along with the context that motivated it and the consequences we accept.
We keep them so that "why on earth is it done this way?" has an answer
that's not "git blame the file and read commits".

## Format

Each ADR is `NNNN-short-name.md`. We use a slimmed Michael Nygard format:

```
# NNNN: Short, declarative title

## Status
Proposed | Accepted | Superseded by ADR-XXXX | Deprecated

## Context
What problem are we solving? What constraints apply? What did we know
when we made the call?

## Decision
What did we choose? Concrete, present tense, declarative.

## Consequences
What gets easier? What gets harder? What did we explicitly accept
trading away?
```

## When to write a new ADR

Write an ADR when:

- A design call would surprise a reader who isn't paged in on the
  conversation that produced it.
- We chose Option A over Option B for non-obvious reasons.
- A future contributor might be tempted to "fix" the current design.
- A constraint (perf, memory, license, ecosystem) drove the call.

Don't write an ADR for:

- Routine implementation choices ("use HashMap not BTreeMap here").
- Things that are obvious from reading the code in five minutes.

## When to supersede

Don't edit historical ADRs except for typos. If we change a decision,
write a new ADR that references the old one with `Superseded by`. The
graveyard is part of the value — it shows how thinking evolved.

## Index

- [0001 — Prism as the parser](0001-prism-as-parser.md)
- [0002 — Bytecode VM, not a JIT](0002-bytecode-vm-not-jit.md)
- [0003 — Hybrid Rc + mark-sweep GC](0003-rc-plus-mark-sweep-hybrid-gc.md)
- [0004 — Block locals share parent's Rc](0004-block-locals-share-parent-rc.md)
- [0005 — Pinned stack for native-driven loops](0005-pinned-stack-for-native-driven-loops.md)
- [0006 — Global string interner with SymId](0006-global-string-intern.md)
- [0007 — Host embedding API](0007-host-embedding-api.md)
- [0008 — Resource caps for untrusted scripts](0008-resource-caps-for-untrusted-scripts.md)
- [0009 — C-ext crate panic policy](0009-cext-panic-policy.md)
- [0010 — Metaprogramming PoC: alias_method, method_missing, define_method](0010-metaprogramming-poc.md)
- [0011 — CRuby-mirrored vm.rs split](0011-cruby-mirrored-vm-split.md)
- [0012 — Thin LTO in release profile](0012-thin-lto-release-profile.md)
- [0013 — CURRENT_VM_PTR borrow-aliasing policy](0013-current-vm-ptr-aliasing.md)
- [0014 — Embed API v2 — `HostCtx` for heap-y arg reads](0014-embed-api-v2-host-ctx.md)
- [0015 — Concentric architecture via tiered Cargo features](0015-concentric-architecture.md)
- [0016 — Method-object reflection chain](0016-method-reflection-chain.md)
- [0017 — Tier-1 boundary specification](0017-tier1-boundary.md)
- [0018 — Workspace migration plan for the concentric architecture](0018-workspace-migration.md)
- [0019 — Tier 2 / Tier 3 boundary specification](0019-tier2-tier3-boundary.md)
- [0020 — Encoding placement — hybrid Tier 1 tag + Tier 2 full registry](0020-encoding-placement.md)
- [0021 — OutputSink trait — no_std-compatible stdout abstraction](0021-output-sink-trait.md)
- [0022 — `_http_server` battery — Rust HTTP front, Ruby app handler](0022-http-server-battery.md)
- [0023 — True async streaming for `_http_server` — architecture analysis](0023-true-async-streaming.md)
- [0024 — Bytecode-level iter drivers + block-break propagation through `Op::Yield`](0024-bytecode-iter-and-block-break.md)
- [0025 — Signal handling + interruptible Vm primitives](0025-signal-handling-interruptible-primitives.md)
- [0026 — Omakase blessed-gem menu](0026-omakase-blessed-gem-menu.md)
- [0027 — `_sqlite` battery — single-conn rusqlite wrapper + Sequel-lite DSL](0027-battery-sqlite.md)
- [0028 — `_socket` battery — blocking std::net TCP backing pure-Ruby Net::HTTP](0028-battery-socket.md)
- [0029 — `_openssl` battery — rustls TLS-client slice for Net::HTTP https](0029-battery-openssl.md)
- [0030 — Closure-threading JIT tier (PoC)](0030-jit-tier.md)
- [0031 — `do_call` dispatch-core optimization](0031-dispatch-core.md)
- [0032 — Native (Cranelift) JIT — surpassing CRuby](0032-jit-native-surpass.md)
- [0033 — Lean VM core (superseded by 0034)](0033-lean-vm-core.md)
- [0034 — JIT-first: a full method JIT to surpass CRuby + YJIT](0034-jit-first-surpass-yjit.md)
- [0035 — JIT inline object access](0035-jit-inline-object-access.md)
- [0036 — Objects as pointers](0036-objects-as-pointers.md)
- [0037 — Baseline JIT tier: frame-keeping direct-threaded substrate](0037-baseline-jit-tier.md)
