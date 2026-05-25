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
