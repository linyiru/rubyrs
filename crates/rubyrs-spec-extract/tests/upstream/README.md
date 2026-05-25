# Vendored upstream `ruby/spec` snapshots

Unmodified fragments of upstream
[`ruby/spec`](https://github.com/ruby/spec), committed here as
input fixtures for the extractor's end-to-end tests
(`crates/rubyrs-spec-extract/tests/upstream.rs`).

| File | Source path | Snapshot date |
|---|---|---|
| `string_empty_spec.rb` | `core/string/empty_spec.rb` | 2026-05 |
| `string_length_spec.rb` | `core/string/length_spec.rb` | 2026-05 |
| `string_reverse_spec.rb` | `core/string/reverse_spec.rb` | 2026-05 |

**Do not edit** the `*.rb` files — they're the upstream source
of record. The matching `*.expected.rb` files capture what the
extractor produces today; regenerate them with:

```bash
UPDATE_EXPECTED=1 cargo test -p rubyrs-spec-extract --test upstream
```

(committed expected files are the canonical "extractor output";
review the diff before pushing.)

## Why these three

Together they exercise the three real-world states v0.1 has
to handle:

- **`reverse_spec.rb`** — heavy on `should ==`, the
  pattern v0.1 rewrites. Output is mostly cleaned-up `assert_eq`
  calls with a few untranslated `should.equal?` / `should.raise`
  cases passing through. Demonstrates the happy path.

- **`empty_spec.rb`** — entirely predicate matchers
  (`should.empty?` / `should_not.empty?`). v0.1 doesn't rewrite
  these yet, so output is essentially the upstream file minus
  the `require_relative` lines. Demonstrates the "extractor saw
  the file but had nothing to rewrite" case — the output
  parses, but running it through the micro-runner would fail at
  the first matcher call. v0.2 (`should_not == val` +
  predicate matchers) will close this gap.

- **`length_spec.rb`** — single-`it_behaves_like` redirect.
  v0.1 leaves that line untouched. Pinned here so the v0.4
  shared-examples inliner (when we get there) has a baseline
  to diff against.
