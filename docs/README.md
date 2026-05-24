# rubyrs docs

Documentation lives next to the code, organised by audience.

## For users (read in this order)

- **[SUBSET.md](SUBSET.md)** — exactly what Ruby semantics rubyrs does and does
  not support today. Read this *first* if you are wondering "will my Ruby
  program run?".
- **[DEVELOPMENT.md](DEVELOPMENT.md)** — building, running, the WASM target,
  troubleshooting.
- **[BENCHMARKS.md](BENCHMARKS.md)** — performance numbers, how they were
  produced, what they mean.

## For contributors

- **[../CONTRIBUTING.md](../CONTRIBUTING.md)** — PR flow, coding style, how to
  add a new built-in or AST node.
- **[ARCHITECTURE.md](ARCHITECTURE.md)** — how the runtime is laid out:
  Prism → IR → bytecode → VM, the heap, the dispatch loop.
- **[TESTING.md](TESTING.md)** — the testing philosophy and the ruby/spec
  ingestion plan. **Important**: this is how we keep ourselves honest.
- **[ROADMAP.md](ROADMAP.md)** — what we are working on next and why.
- **[adr/](adr/)** — Architecture Decision Records. Each non-trivial design
  call has a short doc explaining the *why*, not just the *what*. Read these
  before proposing structural changes.

## Reading order if you are new

1. README at the repo root (5 minutes)
2. SUBSET.md (5 minutes)
3. ARCHITECTURE.md (10 minutes)
4. TESTING.md (5 minutes)
5. Browse adr/ if a particular design choice puzzles you.
