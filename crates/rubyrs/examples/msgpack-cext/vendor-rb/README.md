# Vendored msgpack-ruby Ruby sources

Frozen copy of `lib/msgpack/bigint.rb` from
[msgpack-ruby](https://github.com/msgpack/msgpack-ruby) version 1.7.5,
included to exercise the `.rb`-require path against real upstream
code rather than a hand-rolled equivalent.

License: msgpack-ruby is Apache-2.0; see the upstream `LICENSE`
file for the canonical text. We don't modify the vendored file —
any rubyrs-side adaptations live in the test driver, not here.

Currently vendored:
- `msgpack/bigint.rb` — `MessagePack::Bigint.{to,from}_msgpack_ext`
  helpers used by the `cext_msgpack_bigint` integration test.
- `msgpack/timestamp.rb` — `MessagePack::Timestamp.{new, from_msgpack_ext,
  to_msgpack_ext}`. The class is the Tier-1-friendly Time
  replacement [ADR 0017](../../../../../docs/adr/0017-tier1-boundary.md)
  cites: it holds `(sec, nsec)` integer pairs and uses
  pack/unpack-only wire format. Loads cleanly after PR #89
  (lexical constant scoping → `MessagePack::Timestamp`
  resolves) and the bare-`new` inside-class-singleton-method
  fix (so `from_msgpack_ext`'s `new(sec, 0)` factories work
  without rewriting). Exercised by
  `tests/diff/cext_msgpack_timestamp.rb`.
- `msgpack/buffer.rb`, `msgpack/packer.rb`, `msgpack/unpacker.rb`,
  `msgpack/factory.rb` — the Ruby halves of msgpack-ruby's
  cext-backed `Buffer` / `Packer` / `Unpacker` / `Factory`
  classes. Each opens `module MessagePack; class X; end; end`
  and adds pure-Ruby helpers (`register_type`,
  `registered_types`, `type_registered?`, `Factory#load` /
  `#dump` / `#pool` etc.) on top of the implementations the
  C extension fills in at `require 'msgpack/msgpack'` time.
  Loaded together by `tests/diff/cext_msgpack_pure_ruby_load.rb`
  — proves the pure-Ruby halves parse + define their classes
  + carry the expected method tables. Functional pack /
  unpack round-trips still need the cext (separate scope).
