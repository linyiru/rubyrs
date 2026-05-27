# 0020: Encoding placement — hybrid Tier 1 tag + Tier 2 full registry

## Status

Proposed (2026-05-27). Required by ADR 0019 v3, which
hard-blocks textual batteries (`_csv`, `_yaml`, `_csv_native`,
`_yaml_native`) until this ADR resolves.

## Context

Today's `RStr` (the heap representation behind `Value::Str`,
defined at `crates/rubyrs/src/value.rs:18`) is:

```rust
pub struct RStr {
    pub(crate) content: RefCell<Vec<u8>>,
    pub(crate) frozen: Cell<bool>,
}
```

The bytes are stored raw; UTF-8 is a **soft invariant** (held
by literal sources and `String::into_bytes`, but broken by
cext binary input via `from_bytes`). There is no encoding
tag and no transcoding machinery. All string operations
either:

- treat bytes as UTF-8 lossy (`with_str_lossy`,
  `to_string_lossy`) — substituting U+FFFD for invalid
  sequences, OR
- operate on bytes directly (length-in-bytes, byte slicing).

CRuby's `String` carries an `Encoding` reference per
instance. The encoding affects `String#length` (codepoint
count under that encoding), `#each_char`, comparison
(`String#==` for differently-encoded strings with same
bytes is false), `#force_encoding`, `#encode` (transcoding),
and most regex behaviour. There are ~40 encodings in CRuby's
`encdb`.

ADR 0019 v3 named two pressures:

1. **`_csv` reading Latin-1 / Shift-JIS files** cannot
   round-trip through UTF-8-only strings. The byte path
   works, but `row[0].length` and `row[0][3..5]` give wrong
   answers under any encoding except UTF-8.
2. **Rule 6 parity contract** — pure `_csv` and
   `_csv_native` must agree. If pure `_csv` cannot
   represent Latin-1, neither can the native accelerator.

Decision is required before:
- The first textual battery PR (per ADR 0019 v3 block list)
- Any expansion of `RStr` for other reasons (a future
  `interned` flag, a `taintable` flag, etc.) — extending
  the layout twice is twice the migration cost

## Decision

Adopt a **hybrid**: Tier 1 carries a minimal encoding tag;
the full multi-encoding registry, transcoding tables, and
`Encoding` Ruby class live in Tier 2 behind a new
`_encoding_full` feature.

### Tier 1 extension to `RStr`

Add a single byte-sized tag field:

```rust
pub struct RStr {
    pub(crate) content: RefCell<Vec<u8>>,
    pub(crate) frozen: Cell<bool>,
    pub(crate) encoding: Cell<EncodingTag>,  // NEW
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EncodingTag {
    /// Default for string literals, interpolation results,
    /// String.new from UTF-8 input, anything where the
    /// runtime knows the bytes are UTF-8.
    Utf8 = 0,
    /// CRuby ASCII-8BIT. For cext binary input, raw socket
    /// reads, msgpack frames. Tier 1 string operations
    /// treat as opaque bytes — no codepoint semantics.
    Binary = 1,
    /// Anything else — Latin-1, Shift-JIS, Windows-1252, …
    /// `u8` indexes into the Tier 2 registry. Tier 1
    /// CANNOT transcode `Other(_)` values; operations
    /// requiring codepoint interpretation trap with
    /// `EncodingCompatibilityError` when `_encoding_full`
    /// is disabled.
    Other(u8) = 2,
}
```

Layout cost: 2 bytes per `RStr` (1 byte tag + 1 byte
discriminant for `Other`). For `Other`-less strings (the
overwhelming majority), niche optimisation can reduce this
to 1 byte — implementation detail, not load-bearing.

### Tier 1 semantics with the tag

Operations on UTF-8 / Binary strings are unchanged from
today's behaviour. Operations on `Other(_)` strings without
`_encoding_full` enabled:

| Op | UTF-8 / Binary | Other(_) without _encoding_full |
|---|---|---|
| `bytes` / `bytesize` | Works (raw bytes) | Works (raw bytes) |
| `length` / `size` | UTF-8: char count; Binary: bytes | **Returns byte count + emits `EncodingWarning`** (best-effort fallback) |
| `==` | Bytes equal AND tags compatible (UTF-8 ↔ Binary if ASCII-only) | Bytes-equal AND tags-equal; cross-tag compares to false |
| `concat` (`+`) | OK | OK only if both operands same tag; else `EncodingCompatibilityError` |
| `each_char` / `chars` | Works | **`EncodingCompatibilityError`** without `_encoding_full` |
| `force_encoding(:utf8)` | Works | Works — flips the tag without transcoding bytes |
| `encode(:utf8)` (transcode) | Identity for UTF-8 | **`NotImplementedError(_encoding_full required)`** |

This is the **subset compatible** with the Tier 1 boundary:
Tier 1 can SEE the encoding tag (so `String#encoding` works)
and respects it for compatibility checks, but cannot
transcode `Other(_)` payloads. Embedders who only handle
UTF-8 + Binary pay zero cost; embedders who need Latin-1
opt into `_encoding_full`.

### Tier 2 — `_encoding_full` feature

`crates/rubyrs-language/` (Phase 3 of ADR 0018) gains a
new `_encoding_full` feature that provides:

- **`Encoding` Ruby class** — `Encoding::UTF_8`,
  `Encoding::Shift_JIS`, `Encoding::Windows_1252`,
  `Encoding::ISO_8859_1` (Latin-1), `Encoding::ASCII_8BIT`,
  etc. Implements `Encoding.find(name)`, `Encoding.list`,
  `Encoding.name_list`.
- **Encoding registry** — maps `EncodingTag::Other(u8)`
  indices to `Encoding` objects. Initial v1 set: 8
  encodings covering 95% of real-world non-UTF-8 inputs
  (Latin-1, Latin-9, Windows-1252, Shift_JIS, EUC-JP,
  GBK, Big5, KOI8-R). Adding more is a per-PR cost
  decision; the registry is feature-gated to keep the
  table size bounded.
- **Transcoding tables** — bidirectional conversion
  between any pair of registered encodings via UTF-8 as
  the pivot. Estimated table size: ~600 KB stripped
  (verified via the `encoding_rs` crate's release
  binary cost — vendored, not via dep, to avoid pulling
  the WHATWG-specific quirks `encoding_rs` ships).
- **`String#encode(target_enc)`** — full transcoding,
  including `:replace` / `:undef` options.
- **`String#each_char`, `#chars`, `#length`** for
  `Other(_)` strings — uses the registry to compute
  codepoints under that encoding.
- **Regex encoding interplay** — out of scope for this
  ADR; the existing `regex` feature stays UTF-8-only.
  Cross-encoding regex is a separate concern that, if it
  ever lands, gets its own ADR. ASCII-only regex on
  Latin-1 strings just works (every byte is unambiguous).

`_encoding_full` is NOT in `cli-defaults`. It's pulled in
by `_csv` (which depends on it) and individually by
embedders who need it. Adding it to `everything` is
automatic once `_csv` is in.

### Hard-blocked batteries get unblocked

ADR 0019 v3's block list lifts once this ADR ratifies:

- `_csv` (pure-Ruby) — depends on `_encoding_full`
- `_csv_native` — depends on `_encoding_full`; matches
  pure-Ruby behaviour modulo class-`h` deviations for
  edge cases (CRLF handling under encodings where `\r\n`
  isn't unique)
- `_yaml` (pure-Ruby) — depends on `_encoding_full`
- `_yaml_native` — depends on `_encoding_full`

These batteries can ship in any order once
`_encoding_full` is implemented.

### Migration plan

`RStr` extension is a Tier 1 change touching every
`Value::Str` construction site. Phasing:

**Phase E1 (this ADR + tag-only)**:
- Add `encoding: Cell<EncodingTag>` to `RStr`.
- All construction sites default to `Utf8` (literals,
  interpolation, `String.new(s: String)`,
  `from_bytes`-callers that know they have UTF-8) or
  `Binary` (`from_bytes` from cext binary protocols,
  socket reads).
- Add `String#encoding` returning a symbol
  (`:UTF_8` / `:BINARY` / `:OTHER`). Real `Encoding`
  objects defer to `_encoding_full`.
- Add `String#force_encoding(sym)` for the three Tier 1
  values; trapping for `Other(_)` without
  `_encoding_full`.
- Update `String#==` to enforce tag-compatibility (UTF-8
  ↔ Binary is compatible iff ASCII-only; else
  tag-equal required).
- Update `String#+` / `concat` for tag compatibility.
- Diff_cruby fixtures for the new ops.

**Phase E2 (after Phase E1, gated by ADR 0018 Phase 3)**:
- `_encoding_full` feature lands in `rubyrs-language`.
- Encoding registry + transcoding tables.
- `Encoding` Ruby class.
- Cross-encoding String ops (`#each_char`, `#length`,
  `#encode`).
- Diff_cruby fixtures for textual encoding round-trips.

**Phase E3 (after E2)**:
- `_csv`, `_yaml` batteries land per ADR 0019 v3's
  per-battery ADR requirement.

### What this is not

- **Not a commitment to full CRuby encoding coverage.**
  CRuby ships ~40 encodings; we ship 8 in v1 of
  `_encoding_full`. Adding more is per-PR.
- **Not a regex-encoding answer.** The `regex` feature
  stays UTF-8-only. Cross-encoding regex is a separate
  problem.
- **Not a Tier 4 ABI promise.** CRuby's
  `rb_enc_get(VALUE)` cext API is Tier 4 territory; this
  ADR specifies the rubyrs-internal representation, not
  the cext-visible shape.
- **Not a niche-optimisation commitment.** The 2-byte
  tag may shrink to 1 byte via niche-optimised enum
  layout; if it doesn't, the cost is acceptable.

## Consequences

### What gets easier

- **Textual batteries are unblocked.** `_csv`, `_yaml`,
  and pure-Ruby/native pairs all have a clear
  dependency (`_encoding_full`) and a known semantic
  contract.
- **CRuby semantics improve incrementally.**
  `String#encoding` matches CRuby's return shape
  immediately (Phase E1). `String#==` becomes
  CRuby-correct for cross-encoding cases.
- **Embed users still pay nothing for encoding.** A 2-byte
  tag per `RStr` is the only Tier 1 cost; the
  transcoding tables + Encoding class are Tier 2.
- **cext bridge gets correct.** Today's `from_bytes`
  callers (msgpack binary frames, etc.) default to
  `Binary` instead of being misclassified as UTF-8. The
  cext panic policy (ADR 0009) gets a cleaner story for
  binary-tainted strings.

### What gets harder

- **Every `RStr` construction site needs auditing.**
  Phase E1 is ~150 sites across the codebase (estimated
  from `grep -rn 'RStr::new\|RStr::from_bytes\|RStr {' crates/rubyrs/src`).
  Each needs an explicit tag (Utf8 or Binary). Most are
  Utf8; cext / socket / file-read sites are Binary.
- **`String#==` semantics change.** Today two strings
  with identical bytes are always equal. Post-Phase-E1,
  a UTF-8 `"abc"` and a `Latin-1` `"abc"` (same bytes,
  different tags) are NOT equal — matches CRuby but is a
  silent behaviour change. Diff_cruby fixtures will
  catch any over-broad equality use.
- **Tag-coherence in non-trivial ops.** `gsub` /
  `sub` on a `Latin-1` string with a `UTF-8`
  replacement needs an explicit rule (CRuby: implicit
  transcode-or-trap). Phase E1 traps for cross-tag
  modifications; Phase E2 lifts to CRuby's
  transcode-aware behaviour when `_encoding_full` is on.
- **Hash / equality keys.** `Hash` keys today compare on
  raw bytes via `RStr::content` equality. Post-Phase-E1,
  the tag participates in key equality. Pre-existing
  hashes built from cext-`Binary` strings and
  Ruby-literal `Utf8` strings with the same bytes will
  see new collisions. Audit + diff_cruby fixtures
  required.
- **`_encoding_full`'s ~600 KB table cost.** Adds to
  `cli-defaults` size (via `_csv` dep) once `_csv` is
  promoted. Budget impact: well under the 40 MB
  `cli-defaults` ceiling per ADR 0019 v3 Part D.

### What we explicitly accept trading away

- **Bit-for-bit `RStr` layout stability.** Adding the
  encoding field is a Value-layout change. Cext crates
  that embedded `RStr`'s exact layout (none should —
  the layout is `pub(crate)` — but if any do via
  `mem::transmute` tricks) will break. Acceptable
  before 1.0.
- **Full CRuby encoding parity in v1.** 8 encodings is
  ~95% real-world coverage; the long tail (TIS-620,
  ARMSCII, ISO-2022-* multibyte) is opt-in
  expansion.
- **A simpler "always UTF-8" world.** The current
  no-tag world is simpler. We gain CRuby compatibility
  and CSV correctness at the cost of a soft
  conceptual-load increase. Embedders who never touch
  non-UTF-8 will rarely notice.

## Alternatives considered

1. **Stay UTF-8 only forever.** Block `_csv`, `_yaml`, and
   any non-UTF-8 producer from ever shipping. Trades the
   batteries-included story for layout purity. Rejected
   — ADR 0019's Tier 3 story requires textual batteries
   to be a real option.

2. **Push the tag entirely to Tier 2 (`rubyrs-language`
   carries it via a Side Table).** A `HashMap<*const RStr, EncodingTag>`
   in `rubyrs-language` mapping each `RStr` to its
   encoding. Tier 1 still UTF-8 only at the layout level.
   Rejected:
   - GC integration nightmare (side-table entries must
     follow String lifecycle exactly)
   - `String#encoding` in Tier 1 code can't see Tier 2's
     table (Tier 1 can't import outward per Rule 5)
   - Performance: every string op needs a `HashMap`
     lookup instead of a field read

3. **Tag at the `Value` level instead of `RStr`.** Add an
   `EncodingTag` argument to every `Value::Str` variant
   (e.g. `Value::Str(Rc<RStr>, EncodingTag)`). Rejected:
   - `Value` already has 16 variants; adding a 2-byte
     companion to one of them is layout chaos
   - `RStr`'s mutable interior (`content: RefCell`) means
     the tag MUST live with the bytes — `force_encoding`
     wouldn't otherwise propagate across `Rc::clone`s

4. **Use `encoding_rs` crate directly.** WHATWG-spec
   encoding library, ~600 KB binary cost. Pull as a
   dep behind `_encoding_full`. Rejected (vendor the
   tables, not the crate): `encoding_rs` is WHATWG-spec
   (web-platform compat), which has quirks specific to
   browser encoding label handling that diverge from
   Ruby's stdlib. Vendoring our own minimal table per
   the 8-encoding set keeps semantics aligned with
   CRuby.

5. **Niche-optimised single byte for the tag.** Make
   `EncodingTag` a `Cell<u8>` with `0 = Utf8`,
   `1 = Binary`, `2..=255 = Other(n-2)`. Saves 1 byte
   per `RStr`. Considered but deferred to the
   implementation PR — language design (the enum +
   semantics) doesn't change. The PR can choose either
   layout based on `cargo bloat` measurements.

## Related

- [ADR 0017 — Tier 1 boundary](0017-tier1-boundary.md)
  — Rule 1 (deterministic) is the key constraint
  encoding semantics must respect. Tag-aware ops are
  deterministic from script inputs (the tag IS part of
  the input).
- [ADR 0019 — Tier 2 / Tier 3 boundary](0019-tier2-tier3-boundary.md)
  — v3 hard-blocks textual batteries on this ADR
  resolving. v3 Rule 6 (pure-Ruby canonical) requires
  `_csv` and `_csv_native` to agree on encoding
  behaviour — only possible once `_encoding_full`
  exists.
- [ADR 0018 — Workspace migration plan](0018-workspace-migration.md)
  — Phase 3 (`rubyrs-language` extraction) carries
  `_encoding_full`. Phase E2 lands inside or after
  ADR 0018 Phase 3.
- [ADR 0009 — cext panic policy](0009-cext-panic-policy.md)
  — cext-tainted strings (msgpack binary frames, raw
  socket bytes) default to `Binary` tag; the cext
  bridge needs an audit pass when Phase E1 lands.
- [CRuby `String#encoding` docs](https://docs.ruby-lang.org/en/master/String.html#method-i-encoding)
  — external reference for the semantic contract Phase
  E1 implements
- [CRuby `Encoding` class docs](https://docs.ruby-lang.org/en/master/Encoding.html)
  — external reference for the `_encoding_full`
  Ruby-class API
