# Vendored upstream `ruby/spec` snapshots

Unmodified fragments of upstream
[`ruby/spec`](https://github.com/ruby/spec), committed here as
input fixtures for the extractor's end-to-end tests
(`crates/rubyrs-spec-extract/tests/upstream.rs`).

## Provenance

Vendored from `ruby/spec` at upstream commit
[`448cb34000b160396d6292af77a319f3a600b7ce`](https://github.com/ruby/spec/tree/448cb34000b160396d6292af77a319f3a600b7ce)
(master HEAD on 2026-05-25).

| File | Upstream path | Blob SHA |
|---|---|---|
| `string_empty_spec.rb` | [`core/string/empty_spec.rb`](https://github.com/ruby/spec/blob/448cb34000b160396d6292af77a319f3a600b7ce/core/string/empty_spec.rb) | `8e53a16a` |
| `string_length_spec.rb` | [`core/string/length_spec.rb`](https://github.com/ruby/spec/blob/448cb34000b160396d6292af77a319f3a600b7ce/core/string/length_spec.rb) | `98cee1f0` |
| `string_reverse_spec.rb` | [`core/string/reverse_spec.rb`](https://github.com/ruby/spec/blob/448cb34000b160396d6292af77a319f3a600b7ce/core/string/reverse_spec.rb) | `e37c1125` |

## License

`ruby/spec` carries the
[MIT-style permission notice in its repo root LICENSE file](https://github.com/ruby/spec/blob/master/LICENSE)
(Copyright © 2008 Engine Yard, Inc.). The files vendored here
retain that license; their copyright belongs to the upstream
contributors, not this project. We commit them verbatim
specifically so the extractor has a stable input to test
against and so the diff against upstream is readable.

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
