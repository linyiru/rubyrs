//! Bootsnap-style preamble bytecode cache (`preamble-cache` feature).
//!
//! `Runtime::new` spends ~2.7 ms of the CLI's ~5 ms cold start in
//! the pure source→bytecode pipeline (Prism parse → AST translation
//! → `compile_proto`) over the ~176 KB always-on preamble. That
//! pipeline is deterministic for a given binary, so its output —
//! the interner additions, the `Proto` table, and the per-chunk
//! entry indices — is serialized to a host-provided cache directory
//! on first construction and restored on subsequent ones. Preamble
//! EXECUTION (which builds class/method tables and may consult
//! host `Config` capabilities) still happens live on every
//! construction; only compilation is cached.
//!
//! ## Why the cache can never serve stale bytecode
//!
//! The cache key hashes the current executable's identity (length +
//! mtime, via `std::env::current_exe`) plus the crate version plus
//! the PRE-preamble interner contents (which vary with
//! `Config::load_paths` seeding — see `cache_key`). Preamble
//! sources are `include_str!`-baked into the executable, and the
//! bytecode format is whatever this build's `Op`/`Proto` layout
//! is — both are covered by the exe identity, so a blob is only
//! ever decoded by the exact binary that encoded it. Any mismatch
//! (different build, different pre-state, corrupt file, partial
//! write) falls back to the live compile path silently: the cache
//! is a pure fast-path, never a correctness dependency.
//!
//! ## Blob format (v5): checksummed hybrid POD-region body
//!
//! v4 postcard-decoded the whole snapshot (~0.7-0.9 ms warm-HIT,
//! dominated by per-op serde-enum dispatch and thousands of small
//! String allocations). v5 splits the body into:
//!
//! * a fixed-offset **POD section**: per-proto scalar mirrors
//!   ([`ProtoHot`]), all protos' `code` ops and `op_spans`
//!   concatenated into contiguous regions, and length-prefixed
//!   string regions (proto strings + interner) — restored by raw
//!   byte copy instead of serde;
//! * a **postcard tail** ([`SnapshotCold`]) for the rarely-populated
//!   remainder (kw defaults, byte literals, const chains, sources).
//!
//! The header carries a whole-body **checksum**, verified before any
//! decode: a flipped bit or truncated file (the pre-v5 verifier hole)
//! now degrades to a silent live-compile fallback rather than a
//! panic / wrong decode.
//!
//! ### Soundness of the raw-byte restore
//!
//! Copying `Op` values through raw bytes reads enum padding at
//! encode time and conjures enum values from bytes at decode time —
//! abomonation-territory that is sound ONLY because of the
//! same-binary invariant above (exe identity in the key ⇒ identical
//! `Op` layout, discriminants, cfg-variant set, endianness) plus the
//! body checksum (⇒ the bytes decoded are bit-identical to the bytes
//! a live `Vec<Op>` produced). See [`RawPod`] for the full invariant
//! statement; every `unsafe` block in this file routes through it.
//!
//! ## Capability posture (ADR 0017)
//!
//! Library `Runtime`s never touch the filesystem: the cache only
//! engages when the host sets `Config::preamble_cache_dir`. The
//! CLI binary opts in (defaulting to `$RUBYRS_CACHE_DIR` /
//! `$XDG_CACHE_HOME/rubyrs` / `~/.cache/rubyrs`); `RUBYRS_NO_PREAMBLE_CACHE=1`
//! turns it back off. This knob is deliberately separate from
//! `Config::allow_filesystem_io`, which gates SCRIPT-level IO —
//! providing a cache directory is itself the host's consent.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::bytecode::{Op, Proto};
use crate::error::Span;
use crate::intern::SymId;
use crate::value::Value;
use crate::vm::Vm;

/// Sentinel in `steps` marking the point where
/// `install_kernel_builtins` + `install_basic_object_builtins`
/// run between preamble chunks (they intern method names, so
/// their position in the sequence is order-significant).
pub(crate) const STEP_INSTALL_BUILTINS: u32 = u32::MAX;

const MAGIC: &[u8; 4] = b"RBPC";
const FORMAT_VERSION: u32 = 5; // bumped: hybrid POD-region body + whole-body checksum
/// MAGIC(4) + FORMAT_VERSION(4) + key(8) + body checksum(8).
const HEADER_LEN: usize = 24;

// ---------- checksum ----------

/// Fast non-cryptographic checksum over the blob body. Four
/// independent FxHash-style lanes over 32-byte stripes (the 4-way
/// split breaks the serial multiply dependency chain, ~4× the
/// single-lane throughput — a single-lane byte loop would cost more
/// than the decode it protects), folded together with the tail bytes
/// and the length. Detects the bit-flip / truncation / torn-write
/// corruption class; NOT collision-resistant against adversaries —
/// the cache directory is host-trusted (ADR 0017: providing it is
/// the host's consent), so the threat model is media corruption,
/// not malice.
fn body_checksum(bytes: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95; // FxHash's multiplier
    #[inline]
    fn mix(h: u64, v: u64) -> u64 {
        (h.rotate_left(5) ^ v).wrapping_mul(K)
    }
    // Arbitrary distinct odd seeds so lanes don't collapse together.
    let mut lanes: [u64; 4] = [
        0x9e37_79b9_7f4a_7c15,
        0x6c62_272e_07bb_0143,
        0xcbf2_9ce4_8422_2325,
        0x2545_f491_4f6c_dd1d,
    ];
    let mut chunks = bytes.chunks_exact(32);
    for c in &mut chunks {
        for (i, lane) in lanes.iter_mut().enumerate() {
            *lane = mix(*lane, u64::from_le_bytes(c[i * 8..i * 8 + 8].try_into().unwrap()));
        }
    }
    let mut h = lanes[0];
    h = mix(h, lanes[1]);
    h = mix(h, lanes[2]);
    h = mix(h, lanes[3]);
    for &b in chunks.remainder() {
        h = mix(h, b as u64);
    }
    mix(h, bytes.len() as u64)
}

// ---------- raw-POD plumbing ----------

/// Marker for types the POD regions may contain.
///
/// # Safety invariant (the keystone of the v5 format)
///
/// Implementors are stored/restored by RAW MEMORY COPY. This is
/// sound only under ALL of:
///
/// 1. **Same-binary**: the blob is decoded by the exact executable
///    that encoded it. Guaranteed by `cache_key` hashing exe
///    length+mtime+crate version — a different build (different
///    rustc, features, or source) produces a different key and
///    therefore a different cache file name. Same layout, same
///    enum discriminants, same cfg-variant set, same endianness.
/// 2. **Bit-identical bytes**: the body checksum (verified before
///    any decode) makes the decoded bytes bit-identical to bytes
///    produced by reading real, live values of `Self` at encode
///    time — so every conjured value is a byte-copy of a value
///    that legally existed.
/// 3. **No indirection**: `Self` contains no pointers, references,
///    `Rc`s, or any owner of heap memory — payloads are plain
///    scalars (`i64`/`f64`/`u32`/`u16`/`u8`/`SymId`). `Copy` is
///    required but NOT sufficient (e.g. `&'static str` is `Copy`);
///    each impl below asserts this per-type.
///
/// Residual caveat, documented deliberately: for `Op` (an enum with
/// niche/padding bytes) the encode-side `&[u8]` view reads
/// uninitialized padding, which the Rust abstract machine does not
/// bless even when the concrete bytes are stable — the same
/// technique abomonation uses. Miri would flag it (miri does not run
/// in this repo's CI); on real targets the copy is a plain memcpy of
/// stable heap bytes. The alternative (per-variant tag+payload
/// encode over ~110 `Op` variants) was rejected as strictly worse to
/// maintain for ~0.05 ms of difference.
unsafe trait RawPod: Copy + 'static {}

// SAFETY: `Op` payloads are i64/f64/SymId(u32)/u32/u16/u8 only — no
// heap pointers (bytecode.rs; `LoadConstStrBytes`/`LoadBigInt` etc.
// index side tables rather than embedding Rcs). Discriminant
// validity across the copy is invariant (1)+(2) above.
unsafe impl RawPod for Op {}
// SAFETY: `Span` is `struct { byte_offset: u32 }` — padding-free,
// every bit pattern valid (error.rs).
unsafe impl RawPod for Span {}
// SAFETY: plain integer — padding-free, every bit pattern valid.
unsafe impl RawPod for u32 {}
// SAFETY: `SymId` is `struct(u32)` — padding-free, every bit
// pattern valid (intern.rs; id-validity against the interner is a
// semantic property the verify step owns, not a memory-safety one —
// same rule as SymIds inside `Op` payloads).
unsafe impl RawPod for SymId {}
// SAFETY: `ProtoHot` is #[repr(C)] with an explicit `_pad` field
// making it padding-free (const-asserted below); all fields are
// plain integers, every bit pattern valid (flag/idx consistency is
// re-checked structurally by `decode_body`, not assumed).
unsafe impl RawPod for ProtoHot {}

/// Raw-byte view of a POD slice for writing into the blob body.
///
/// SAFETY: see [`RawPod`] — for `Op` this view includes padding
/// bytes (residual caveat there); for the other impls the types are
/// padding-free so every byte is initialized.
fn pod_bytes<T: RawPod>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), std::mem::size_of_val(v)) }
}

/// Reconstruct a `Vec<T>` by byte-copy out of a blob region.
/// `bytes.len()` must equal `count * size_of::<T>()` (callers carve
/// the region with exactly that length). The destination allocation
/// provides alignment; the source may be unaligned (byte copy).
///
/// SAFETY (caller): `bytes` must be a checksum-verified copy of a
/// region produced by `pod_bytes::<T>` in this same binary — see
/// [`RawPod`] for why that makes the conjured values valid.
unsafe fn pod_vec_from_bytes<T: RawPod>(bytes: &[u8], count: usize) -> Vec<T> {
    debug_assert_eq!(bytes.len(), count * std::mem::size_of::<T>());
    let mut v: Vec<T> = Vec::with_capacity(count);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), v.as_mut_ptr().cast::<u8>(), bytes.len());
        v.set_len(count);
    }
    v
}

// ---------- per-proto POD mirror ----------

// `ProtoHot.flags` bits.
const F_FROZEN_STRING_LITERAL: u16 = 1 << 0;
const F_CREATES_BLOCK: u16 = 1 << 1;
const F_HAS_REST_PARAM: u16 = 1 << 2;
const F_HAS_KW_REST_PARAM: u16 = 1 << 3;
const F_HAS_BLOCK_PARAM: u16 = 1 << 4;
const F_HAS_BLOCK_PARAM_SLOT: u16 = 1 << 5;
const F_HAS_GETTER_IVAR: u16 = 1 << 6;
const F_HAS_SYM_PROC: u16 = 1 << 7;
const F_HAS_BLOCK_SHAPE: u16 = 1 << 8;
const F_BLOCK_SHAPE_REST: u16 = 1 << 9;
const F_BLOCK_SHAPE_KW_REST: u16 = 1 << 10;

/// Fixed-size POD mirror of one `Proto`'s scalar fields plus the
/// lengths that tie it to the shared regions. One entry per proto,
/// raw-copied as a single contiguous array (the bulk of the per-
/// proto decode cost in v4 was serde field dispatch over exactly
/// these ~20 small fields × ~1050 protos).
///
/// String fields are NOT here: each proto's strings are emitted into
/// the shared string region in a fixed documented order —
///
/// > name, params[0..params_len], local_names[0..local_names_len],
/// > rest_param (iff `F_HAS_REST_PARAM`), kw_rest_param (iff
/// > `F_HAS_KW_REST_PARAM`), block_param (iff `F_HAS_BLOCK_PARAM`)
///
/// — and consumed back in the same order at decode
/// (`StrCursor`), so no per-string offsets are stored, only
/// lengths. `Option` presence must come from `flags`, never from a
/// zero length (`def f(*)` really does carry `Some("")`).
///
/// Truly cold per-proto data (kw defaults, byte literals, const
/// chains, lexical scope, block kw params) stays in the postcard
/// tail as [`ProtoCold`].
#[repr(C)]
#[derive(Clone, Copy)]
struct ProtoHot {
    code_len: u32,
    spans_len: u32,
    params_len: u32,
    local_names_len: u32,
    /// Number of `lexical_scope` SymIds this proto consumes from the
    /// shared lex-scope region (consumed sequentially in proto
    /// order, like the string region).
    lexical_scope_len: u32,
    /// Index into `SnapshotCold::filenames` (proto filenames repeat
    /// heavily — ~50 uniques across ~1050 protos — so the decode
    /// side clones one `Rc<str>` per unique instead of allocating
    /// per proto).
    filename_idx: u32,
    /// Valid iff `F_HAS_GETTER_IVAR`.
    getter_ivar: u32,
    /// Valid iff `F_HAS_SYM_PROC`.
    sym_proc_sym: u32,
    sym_proc_cache: u32,
    line_base: i32,
    n_required_positional: u16,
    n_required_post: u16,
    n_locals: u16,
    /// Valid iff `F_HAS_BLOCK_PARAM_SLOT`.
    block_param_slot: u16,
    block_body_local_start: u16,
    n_optional_params: u16,
    /// Valid iff `F_HAS_BLOCK_SHAPE` (with the two bool components
    /// in `F_BLOCK_SHAPE_REST` / `F_BLOCK_SHAPE_KW_REST`).
    block_shape_param_start: u16,
    block_shape_n_params: u16,
    flags: u16,
    /// Explicit tail padding so the struct is padding-free (every
    /// byte of the encoded array is an initialized, deterministic
    /// value). Always 0.
    _pad: u16,
}

/// Padding-free assertion for `ProtoHot` (10×u32/i32 + 10×u16): if a
/// field edit introduces implicit padding, the encode-side byte view
/// would leak uninitialized bytes — fail the build instead.
const _: () = assert!(std::mem::size_of::<ProtoHot>() == 10 * 4 + 10 * 2);

// ---------- postcard tail (cold remainder) ----------

/// Owned (deserialize) shape of one proto's cold remainder. Borrow
/// twin `ProtoColdRef` below is used at encode time (`Proto` is not
/// `Clone`). Field ORDER is load-bearing: postcard is positional.
///
/// Stored SPARSELY (see `SnapshotCold::protos_cold`): only ~200 of
/// ~1050 preamble protos have ANY of these fields non-empty, so the
/// dense encoding was mostly serde dispatch over empty vecs.
#[derive(Default, serde::Deserialize)]
struct ProtoCold {
    kw_param_defaults: Vec<Option<Value>>,
    kw_has_computed_default: Vec<bool>,
    block_kw_params: Vec<(String, u16, bool)>,
    byte_literals: Vec<Rc<[u8]>>,
    const_chains: Vec<Vec<SymId>>,
}

#[derive(serde::Serialize)]
struct ProtoColdRef<'a> {
    kw_param_defaults: &'a [Option<Value>],
    kw_has_computed_default: &'a [bool],
    block_kw_params: &'a [(String, u16, bool)],
    byte_literals: &'a [Rc<[u8]>],
    const_chains: &'a [Vec<SymId>],
}

impl ProtoColdRef<'_> {
    fn is_empty(&self) -> bool {
        self.kw_param_defaults.is_empty()
            && self.kw_has_computed_default.is_empty()
            && self.block_kw_params.is_empty()
            && self.byte_literals.is_empty()
            && self.const_chains.is_empty()
    }
}

/// Owned (deserialize) shape of the postcard tail. Borrow twin
/// `SnapshotColdRef` below is the encode-time shape.
#[derive(serde::Deserialize)]
struct SnapshotCold {
    /// `vm.interner.len()` at `load_preamble` entry when the blob
    /// was stored. Restore verifies the live prefix matches
    /// (length AND contents) before appending the rest — SymIds
    /// are positional, so any prefix drift would mis-bind every
    /// symbol the preamble bytecode references.
    pre_interner_len: u32,
    /// `vm.protos.len()` at `load_preamble` entry (expected 0).
    pre_protos_len: u32,
    /// `vm.cache_counter.call` at preamble completion (sizes the
    /// call inline-cache vector).
    cache_counter: u32,
    /// `vm.cache_counter.ivar` at preamble completion (sizes the
    /// ivar-site cache vector, ADR 0035 Ph4/5).
    ivar_counter: u32,
    /// Replay program: entry proto index per preamble chunk, in
    /// chunk order, with `STEP_INSTALL_BUILTINS` marking the
    /// host-side builtin-install step.
    steps: Vec<u32>,
    /// `vm.sources` entries whose text is `include_str!`-baked into
    /// this binary, as indices into `crate::PREAMBLE_BAKED_SOURCES`
    /// (the exe-identity cache key guarantees encoder and decoder
    /// share the table). ~318 KB of preamble text this blob does NOT
    /// carry.
    baked_sources: Vec<u32>,
    /// Remaining `vm.sources` pairs (filename, source) for backtrace
    /// resolution — the live path inserts these in `eval_inner`.
    /// After the baked split this is only the handful of tiny inline
    /// literals (stdlib autoload registrations) plus any battery
    /// preamble a feature build loads.
    sources: Vec<(String, String)>,
    /// Unique proto filenames, referenced by `ProtoHot::filename_idx`.
    filenames: Vec<String>,
    /// SPARSE cold remainders: `(proto_index, cold)` in strictly
    /// increasing index order, present only for protos with at
    /// least one non-empty cold field. Absent protos restore
    /// `ProtoCold::default()` (all-empty).
    protos_cold: Vec<(u32, ProtoCold)>,
}

#[derive(serde::Serialize)]
struct SnapshotColdRef<'a> {
    pre_interner_len: u32,
    pre_protos_len: u32,
    cache_counter: u32,
    ivar_counter: u32,
    steps: &'a [u32],
    baked_sources: Vec<u32>,
    sources: Vec<(&'a str, &'a str)>,
    filenames: Vec<&'a str>,
    protos_cold: Vec<(u32, ProtoColdRef<'a>)>,
}

// ---------- decode ----------

/// Bounds-checked sequential reader over the blob body. Every carve
/// is `Option` — a truncated or size-lying blob (already unlikely
/// past the checksum; belt-and-braces so even a checksum collision
/// cannot read out of bounds or over-allocate) surfaces as a miss,
/// never a panic.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn rest(self) -> &'a [u8] {
        &self.buf[self.pos..]
    }
}

/// Sequential reader for the length-prefixed string region (see the
/// emission-order contract on [`ProtoHot`]). UTF-8 is re-validated
/// per slice (`str::from_utf8` — SIMD ASCII fast path, negligible),
/// so string reconstruction needs no unsafe.
struct StrCursor<'a> {
    lens: &'a [u32],
    region: &'a [u8],
    next_len: usize,
    off: usize,
}

impl<'a> StrCursor<'a> {
    fn next(&mut self) -> Option<&'a str> {
        let len = *self.lens.get(self.next_len)? as usize;
        self.next_len += 1;
        let end = self.off.checked_add(len)?;
        let b = self.region.get(self.off..end)?;
        self.off = end;
        std::str::from_utf8(b).ok()
    }
    fn fully_consumed(&self) -> bool {
        self.next_len == self.lens.len() && self.off == self.region.len()
    }
}

/// Everything `try_load` needs out of a decoded body, with the
/// proto table already fully reconstructed. Borrows the interner
/// strings straight out of the blob bytes (they are only read once,
/// by the prefix verify + `Interner::intern`, so no intermediate
/// `String`s are allocated for them at all).
struct Decoded<'a> {
    pre_interner_len: u32,
    pre_protos_len: u32,
    cache_counter: u32,
    ivar_counter: u32,
    steps: Vec<u32>,
    baked_sources: Vec<u32>,
    sources: Vec<(String, String)>,
    interner: Vec<&'a str>,
    protos: Vec<Proto>,
}

/// Decode a v5 body (everything after the 24-byte header) into a
/// [`Decoded`]. `None` = structurally inconsistent → cache miss.
/// The caller MUST have verified the body checksum first — the raw
/// POD copies in here rely on it (see [`RawPod`] invariant 2); the
/// structural checks below are the belt-and-braces layer that keeps
/// even a checksum collision memory-safe (bounds, counts, and
/// cross-section lengths all have to agree before any copy).
fn decode_body(body: &[u8]) -> Option<Decoded<'_>> {
    const OP_SIZE: usize = std::mem::size_of::<Op>();
    const SPAN_SIZE: usize = std::mem::size_of::<Span>();
    const HOT_SIZE: usize = std::mem::size_of::<ProtoHot>();

    let mut c = Cursor { buf: body, pos: 0 };
    let n_protos = c.u32()? as usize;
    let total_ops = c.u32()? as usize;
    let total_spans = c.u32()? as usize;
    let total_lex = c.u32()? as usize;
    let n_str_lens = c.u32()? as usize;
    let str_region_len = c.u32()? as usize;
    let n_interner = c.u32()? as usize;
    let interner_region_len = c.u32()? as usize;

    let hot_bytes = c.take(n_protos.checked_mul(HOT_SIZE)?)?;
    let ops_bytes = c.take(total_ops.checked_mul(OP_SIZE)?)?;
    let spans_bytes = c.take(total_spans.checked_mul(SPAN_SIZE)?)?;
    let lex_bytes = c.take(total_lex.checked_mul(std::mem::size_of::<SymId>())?)?;
    let str_lens_bytes = c.take(n_str_lens.checked_mul(4)?)?;
    let interner_lens_bytes = c.take(n_interner.checked_mul(4)?)?;
    let str_region = c.take(str_region_len)?;
    let interner_region = c.take(interner_region_len)?;
    let cold: SnapshotCold = postcard::from_bytes(c.rest()).ok()?;

    let SnapshotCold {
        pre_interner_len,
        pre_protos_len,
        cache_counter,
        ivar_counter,
        steps,
        baked_sources,
        sources,
        filenames,
        protos_cold,
    } = cold;
    // The sparse cold list must be strictly increasing and in range
    // (this also bounds its length by n_protos).
    if !protos_cold
        .windows(2)
        .all(|w| w[0].0 < w[1].0)
        || protos_cold.last().is_some_and(|(i, _)| *i as usize >= n_protos)
    {
        return None;
    }

    // SAFETY: checksum-verified copies of `pod_bytes` output from
    // this same binary (see `RawPod`); region lengths were carved as
    // exactly count × size above.
    let hot: Vec<ProtoHot> = unsafe { pod_vec_from_bytes(hot_bytes, n_protos) };
    let str_lens: Vec<u32> = unsafe { pod_vec_from_bytes(str_lens_bytes, n_str_lens) };
    let interner_lens: Vec<u32> = unsafe { pod_vec_from_bytes(interner_lens_bytes, n_interner) };

    // Cross-section consistency: the per-proto lengths must tile the
    // shared regions exactly.
    let sum_ops: u64 = hot.iter().map(|h| h.code_len as u64).sum();
    let sum_spans: u64 = hot.iter().map(|h| h.spans_len as u64).sum();
    let sum_lex: u64 = hot.iter().map(|h| h.lexical_scope_len as u64).sum();
    if sum_ops != total_ops as u64 || sum_spans != total_spans as u64 || sum_lex != total_lex as u64
    {
        return None;
    }

    // Interner: borrow slices straight out of the blob.
    let mut interner: Vec<&str> = Vec::with_capacity(n_interner);
    let mut off = 0usize;
    for &l in &interner_lens {
        let end = off.checked_add(l as usize)?;
        interner.push(std::str::from_utf8(interner_region.get(off..end)?).ok()?);
        off = end;
    }
    if off != interner_region.len() {
        return None;
    }

    // Reconstruct the proto table.
    let filenames_rc: Vec<Rc<str>> = filenames.iter().map(|s| Rc::from(s.as_str())).collect();
    let mut sc = StrCursor { lens: &str_lens, region: str_region, next_len: 0, off: 0 };
    let mut ops_off = 0usize;
    let mut spans_off = 0usize;
    let mut lex_off = 0usize;
    let mut cold_iter = protos_cold.into_iter().peekable();
    let mut protos: Vec<Proto> = Vec::with_capacity(n_protos);
    for (i, h) in hot.iter().enumerate() {
        let pc = if cold_iter.peek().is_some_and(|(ci, _)| *ci as usize == i) {
            cold_iter.next().unwrap().1
        } else {
            ProtoCold::default()
        };
        let has = |f: u16| h.flags & f != 0;
        let code_bytes = (h.code_len as usize).checked_mul(OP_SIZE)?;
        let span_bytes = (h.spans_len as usize).checked_mul(SPAN_SIZE)?;
        let lex_bytes_n =
            (h.lexical_scope_len as usize).checked_mul(std::mem::size_of::<SymId>())?;
        // In-bounds by the sum check above; `get` keeps it panic-free
        // regardless.
        let code_src = ops_bytes.get(ops_off..ops_off + code_bytes)?;
        let spans_src = spans_bytes.get(spans_off..spans_off + span_bytes)?;
        let lex_src = lex_bytes.get(lex_off..lex_off + lex_bytes_n)?;
        ops_off += code_bytes;
        spans_off += span_bytes;
        lex_off += lex_bytes_n;
        // SAFETY: as for `hot` above — checksummed same-binary bytes,
        // exact count × size slices.
        let code: Vec<Op> = unsafe { pod_vec_from_bytes(code_src, h.code_len as usize) };
        let op_spans: Vec<Span> = unsafe { pod_vec_from_bytes(spans_src, h.spans_len as usize) };
        let lexical_scope: Vec<SymId> =
            unsafe { pod_vec_from_bytes(lex_src, h.lexical_scope_len as usize) };

        let name = sc.next()?.to_owned();
        let params: Vec<String> = {
            let mut v = Vec::with_capacity(h.params_len as usize);
            for _ in 0..h.params_len {
                v.push(sc.next()?.to_owned());
            }
            v
        };
        let local_names: Vec<String> = {
            let mut v = Vec::with_capacity(h.local_names_len as usize);
            for _ in 0..h.local_names_len {
                v.push(sc.next()?.to_owned());
            }
            v
        };
        let rest_param = if has(F_HAS_REST_PARAM) { Some(sc.next()?.to_owned()) } else { None };
        let kw_rest_param =
            if has(F_HAS_KW_REST_PARAM) { Some(sc.next()?.to_owned()) } else { None };
        let block_param = if has(F_HAS_BLOCK_PARAM) { Some(sc.next()?.to_owned()) } else { None };

        protos.push(Proto {
            name,
            params,
            n_required_positional: h.n_required_positional,
            n_required_post: h.n_required_post,
            rest_param,
            kw_param_defaults: pc.kw_param_defaults,
            kw_has_computed_default: pc.kw_has_computed_default,
            kw_rest_param,
            block_param,
            block_kw_params: pc.block_kw_params,
            block_param_slot: has(F_HAS_BLOCK_PARAM_SLOT).then_some(h.block_param_slot),
            n_locals: h.n_locals,
            local_names,
            frozen_string_literal: has(F_FROZEN_STRING_LITERAL),
            line_base: h.line_base,
            // Only runtime eval'd protos ever carry a non-None
            // encoding; cached (preamble) protos restore as None —
            // same rule as the v4 `serde(skip)`.
            source_encoding: None,
            creates_block: has(F_CREATES_BLOCK),
            getter_ivar: has(F_HAS_GETTER_IVAR).then_some(SymId(h.getter_ivar)),
            sym_proc: has(F_HAS_SYM_PROC).then_some((SymId(h.sym_proc_sym), h.sym_proc_cache)),
            // Runtime-only slot cache: restores unfilled (u32::MAX;
            // 0 would be a VALID slot).
            getter_slot: std::cell::Cell::new(u32::MAX),
            code,
            op_spans,
            filename: filenames_rc.get(h.filename_idx as usize)?.clone(),
            block_body_local_start: h.block_body_local_start,
            n_optional_params: h.n_optional_params,
            block_shape: has(F_HAS_BLOCK_SHAPE).then_some((
                h.block_shape_param_start,
                h.block_shape_n_params,
                has(F_BLOCK_SHAPE_REST),
                has(F_BLOCK_SHAPE_KW_REST),
            )),
            byte_literals: pc.byte_literals,
            const_chains: pc.const_chains,
            lexical_scope,
        });
    }
    // Every region must be consumed exactly — leftovers mean the
    // sections disagree about counts.
    if !sc.fully_consumed()
        || ops_off != ops_bytes.len()
        || spans_off != spans_bytes.len()
        || lex_off != lex_bytes.len()
        || cold_iter.next().is_some()
    {
        return None;
    }
    Some(Decoded {
        pre_interner_len,
        pre_protos_len,
        cache_counter,
        ivar_counter,
        steps,
        baked_sources,
        sources,
        interner,
        protos,
    })
}

/// The fields `try_load` hands back for the Runtime to replay.
pub(crate) struct ReplayPlan {
    pub(crate) steps: Vec<u32>,
}

fn fx_hash_bytes(h: &mut crate::intern::FxHasher, bytes: &[u8]) {
    use std::hash::Hasher;
    h.write(bytes);
}

/// Cache key for the current process + pre-preamble state. `None`
/// disables the cache for this construction (e.g. `current_exe`
/// unavailable on the platform).
pub(crate) fn cache_key(vm: &Vm) -> Option<u64> {
    use std::hash::Hasher;
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(&exe).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let mut h = crate::intern::FxHasher::default();
    fx_hash_bytes(&mut h, env!("CARGO_PKG_VERSION").as_bytes());
    h.write_u64(meta.len());
    h.write_u64(mtime.as_secs());
    h.write_u32(mtime.subsec_nanos());
    // Pre-preamble interner contents: `Vm::new`'s pre-interned
    // symbols plus whatever `Config::load_paths` seeding interned
    // (`$LOAD_PATH`). Two Runtimes with different pre-state get
    // different keys and therefore different cache files — both
    // valid, neither poisoning the other.
    h.write_usize(vm.interner.len());
    for i in 0..vm.interner.len() {
        fx_hash_bytes(&mut h, vm.interner.resolve(SymId(i as u32)).as_bytes());
    }
    Some(h.finish())
}

fn cache_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(format!("preamble-{key:016x}.bin"))
}

/// Miss-stage telemetry under `RUBYRS_STARTUP_PROF=1` — names which
/// gate rejected the blob so cache problems are diagnosable without
/// a debugger.
fn dbg_miss(stage: &str) {
    if std::env::var_os("RUBYRS_STARTUP_PROF").is_some() {
        eprintln!("startup-prof: preamble-cache miss at: {stage}");
    }
}

/// Try to restore a snapshot into `vm`. On hit, applies the
/// interner / protos / call-cache sizing / sources and returns the
/// replay plan; the caller runs the plan's entry protos (and the
/// builtin-install sentinel) in order. Any mismatch returns `None`
/// and leaves `vm` untouched, so the caller falls back to the live
/// compile path.
pub(crate) fn try_load(vm: &mut Vm, dir: &Path, key: u64) -> Option<ReplayPlan> {
    // Stage timing is prof-gated: clock reads + ns math are dead
    // cost on every boot otherwise.
    let prof = std::env::var_os("RUBYRS_STARTUP_PROF").is_some();
    let t_read = prof.then(std::time::Instant::now);
    let Ok(bytes) = std::fs::read(cache_file(dir, key)) else { dbg_miss("read"); return None };
    let read_ns = t_read.map_or(0, |t| t.elapsed().as_nanos() as u64);
    if bytes.len() < HEADER_LEN || &bytes[0..4] != MAGIC {
        dbg_miss("magic/short");
        return None;
    }
    if u32::from_le_bytes(bytes[4..8].try_into().ok()?) != FORMAT_VERSION {
        dbg_miss("format version");
        return None;
    }
    if u64::from_le_bytes(bytes[8..16].try_into().ok()?) != key {
        dbg_miss("key");
        return None;
    }
    // Whole-body checksum BEFORE any decode: bit flips / truncation
    // / torn writes fall back to live compile here instead of
    // reaching the decoder (the pre-v5 "blob body unchecksummed"
    // hole), and the raw POD restore below is entitled to assume
    // bit-identical bytes (RawPod invariant 2).
    let t_sum = prof.then(std::time::Instant::now);
    let stored_sum = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    if body_checksum(&bytes[HEADER_LEN..]) != stored_sum {
        dbg_miss("checksum");
        return None;
    }
    let sum_ns = t_sum.map_or(0, |t| t.elapsed().as_nanos() as u64);
    let t_decode = prof.then(std::time::Instant::now);
    let Some(dec) = decode_body(&bytes[HEADER_LEN..]) else {
        dbg_miss("decode");
        return None;
    };
    let decode_ns = t_decode.map_or(0, |t| t.elapsed().as_nanos() as u64);
    let t_apply = prof.then(std::time::Instant::now);
    // Verify the pre-preamble state matches what the blob was
    // stored against. The key already hashes all of this; the
    // explicit re-check is belt-and-braces against hash collision
    // and costs ~50 string compares.
    if vm.protos.len() as u32 != dec.pre_protos_len {
        dbg_miss("pre-protos len");
        return None;
    }
    if vm.interner.len() as u32 != dec.pre_interner_len {
        dbg_miss("pre-interner len");
        return None;
    }
    if dec.interner.len() < dec.pre_interner_len as usize {
        dbg_miss("interner shorter than prefix");
        return None;
    }
    for i in 0..vm.interner.len() {
        if &**vm.interner.resolve(SymId(i as u32)) != dec.interner[i] {
            dbg_miss("interner prefix");
            return None;
        }
    }
    // Baked-source indices must resolve inside this binary's table
    // (checked BEFORE the commit point below — an out-of-range index
    // means a corrupt/foreign blob and must fall back, not panic).
    if dec
        .baked_sources
        .iter()
        .any(|&i| i as usize >= crate::PREAMBLE_BAKED_SOURCES.len())
    {
        dbg_miss("baked-source index out of range");
        return None;
    }
    // Apply. From here on the snapshot is committed — every step
    // below is infallible (or panics on ICE, same as the live
    // path's `.expect`).
    for s in &dec.interner[dec.pre_interner_len as usize..] {
        vm.interner.intern(s);
    }
    debug_assert_eq!(vm.interner.len(), dec.interner.len());
    vm.protos = dec.protos;
    vm.cache_counter = crate::compiler::CidGen { call: dec.cache_counter, ivar: dec.ivar_counter };
    vm.ensure_call_caches(dec.cache_counter as usize);
    vm.ensure_ivar_caches(dec.ivar_counter as usize);
    for &i in &dec.baked_sources {
        let (f, src) = crate::PREAMBLE_BAKED_SOURCES[i as usize];
        vm.sources.insert(Rc::from(f), Rc::from(src));
    }
    for (f, src) in dec.sources {
        vm.sources.insert(Rc::from(f.as_str()), Rc::from(src.as_str()));
    }
    if let Some(t_apply) = t_apply {
        eprintln!(
            "startup-prof: preamble-cache blob={}B read={:.3}ms checksum={:.3}ms decode={:.3}ms verify+apply={:.3}ms",
            bytes.len(),
            read_ns as f64 / 1e6,
            sum_ns as f64 / 1e6,
            decode_ns as f64 / 1e6,
            t_apply.elapsed().as_nanos() as f64 / 1e6,
        );
    }
    Some(ReplayPlan { steps: dec.steps })
}

/// Serialize the post-preamble compile state. Best-effort: any IO
/// or encode failure is swallowed (the cache is an optimisation,
/// and the next construction simply compiles live again).
/// `key` MUST be the pre-preamble key computed at `load_preamble`
/// entry — `cache_key` hashes the interner contents, which by
/// store time include every preamble symbol; recomputing here
/// would produce a key `try_load` (which runs pre-preamble) can
/// never reproduce.
pub(crate) fn store(
    vm: &Vm,
    dir: &Path,
    key: u64,
    pre_interner_len: u32,
    pre_protos_len: u32,
    steps: &[u32],
) {
    let prof = std::env::var_os("RUBYRS_STARTUP_PROF").is_some();
    let t_encode = prof.then(std::time::Instant::now);

    // Split `vm.sources` into baked-table indices and owned pairs.
    // A baked reference requires BOTH the filename and the full
    // source text to match the table entry (content compare, miss
    // path only) — belt-and-braces so a hypothetical divergence
    // degrades to carrying the owned text, never to serving wrong
    // source.
    let mut baked_sources: Vec<u32> = Vec::new();
    let mut owned_sources: Vec<(&str, &str)> = Vec::new();
    for (k, v) in vm.sources.iter() {
        match crate::PREAMBLE_BAKED_SOURCES
            .iter()
            .position(|(name, text)| *name == &**k && *text == &**v)
        {
            Some(i) => baked_sources.push(i as u32),
            None => owned_sources.push((&**k, &**v)),
        }
    }

    // POD section: per-proto scalar mirrors + shared regions. The
    // string emission order here is the contract documented on
    // `ProtoHot` — `decode_body`'s StrCursor consumes in the same
    // order.
    let n_protos = vm.protos.len();
    let mut hot: Vec<ProtoHot> = Vec::with_capacity(n_protos);
    let mut str_lens: Vec<u32> = Vec::new();
    let mut str_region: Vec<u8> = Vec::new();
    let mut filenames: Vec<&str> = Vec::new();
    let mut filename_idx: crate::intern::FxHashMap<&str, u32> = Default::default();
    let mut total_ops = 0u64;
    let mut total_spans = 0u64;
    let mut total_lex = 0u64;
    for p in &vm.protos {
        let mut push_str = |s: &str| {
            str_lens.push(s.len() as u32);
            str_region.extend_from_slice(s.as_bytes());
        };
        let mut flags = 0u16;
        if p.frozen_string_literal {
            flags |= F_FROZEN_STRING_LITERAL;
        }
        if p.creates_block {
            flags |= F_CREATES_BLOCK;
        }
        if p.rest_param.is_some() {
            flags |= F_HAS_REST_PARAM;
        }
        if p.kw_rest_param.is_some() {
            flags |= F_HAS_KW_REST_PARAM;
        }
        if p.block_param.is_some() {
            flags |= F_HAS_BLOCK_PARAM;
        }
        if p.block_param_slot.is_some() {
            flags |= F_HAS_BLOCK_PARAM_SLOT;
        }
        if p.getter_ivar.is_some() {
            flags |= F_HAS_GETTER_IVAR;
        }
        if p.sym_proc.is_some() {
            flags |= F_HAS_SYM_PROC;
        }
        let (bs_start, bs_n) = match p.block_shape {
            Some((a, b, r, k)) => {
                flags |= F_HAS_BLOCK_SHAPE;
                if r {
                    flags |= F_BLOCK_SHAPE_REST;
                }
                if k {
                    flags |= F_BLOCK_SHAPE_KW_REST;
                }
                (a, b)
            }
            None => (0, 0),
        };
        let fidx = *filename_idx.entry(&*p.filename).or_insert_with(|| {
            filenames.push(&*p.filename);
            (filenames.len() - 1) as u32
        });
        total_ops += p.code.len() as u64;
        total_spans += p.op_spans.len() as u64;
        total_lex += p.lexical_scope.len() as u64;
        hot.push(ProtoHot {
            code_len: p.code.len() as u32,
            spans_len: p.op_spans.len() as u32,
            params_len: p.params.len() as u32,
            local_names_len: p.local_names.len() as u32,
            lexical_scope_len: p.lexical_scope.len() as u32,
            filename_idx: fidx,
            getter_ivar: p.getter_ivar.map_or(0, |s| s.0),
            sym_proc_sym: p.sym_proc.map_or(0, |(s, _)| s.0),
            sym_proc_cache: p.sym_proc.map_or(0, |(_, c)| c),
            line_base: p.line_base,
            n_required_positional: p.n_required_positional,
            n_required_post: p.n_required_post,
            n_locals: p.n_locals,
            block_param_slot: p.block_param_slot.unwrap_or(0),
            block_body_local_start: p.block_body_local_start,
            n_optional_params: p.n_optional_params,
            block_shape_param_start: bs_start,
            block_shape_n_params: bs_n,
            flags,
            _pad: 0,
        });
        push_str(&p.name);
        for s in &p.params {
            push_str(s);
        }
        for s in &p.local_names {
            push_str(s);
        }
        if let Some(s) = &p.rest_param {
            push_str(s);
        }
        if let Some(s) = &p.kw_rest_param {
            push_str(s);
        }
        if let Some(s) = &p.block_param {
            push_str(s);
        }
    }

    // Interner region (full table, prefix included, in id order).
    let mut interner_lens: Vec<u32> = Vec::with_capacity(vm.interner.len());
    let mut interner_region: Vec<u8> = Vec::new();
    for i in 0..vm.interner.len() {
        let s = vm.interner.resolve(SymId(i as u32));
        interner_lens.push(s.len() as u32);
        interner_region.extend_from_slice(s.as_bytes());
    }

    // Postcard tail.
    let cold = SnapshotColdRef {
        pre_interner_len,
        pre_protos_len,
        cache_counter: vm.cache_counter.call,
        ivar_counter: vm.cache_counter.ivar,
        steps,
        baked_sources,
        sources: owned_sources,
        filenames,
        protos_cold: vm
            .protos
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let cold = ProtoColdRef {
                    kw_param_defaults: &p.kw_param_defaults,
                    kw_has_computed_default: &p.kw_has_computed_default,
                    block_kw_params: &p.block_kw_params,
                    byte_literals: &p.byte_literals,
                    const_chains: &p.const_chains,
                };
                (!cold.is_empty()).then_some((i as u32, cold))
            })
            .collect(),
    };
    let Ok(cold_bytes) = postcard::to_allocvec(&cold) else { return };

    // Assemble the body, then the checksummed header around it.
    let ops_bytes: usize = total_ops as usize * std::mem::size_of::<Op>();
    let spans_bytes: usize = total_spans as usize * std::mem::size_of::<Span>();
    let lex_bytes: usize = total_lex as usize * std::mem::size_of::<SymId>();
    let mut body: Vec<u8> = Vec::with_capacity(
        32 + n_protos * std::mem::size_of::<ProtoHot>()
            + ops_bytes
            + spans_bytes
            + lex_bytes
            + (str_lens.len() + interner_lens.len()) * 4
            + str_region.len()
            + interner_region.len()
            + cold_bytes.len(),
    );
    for v in [
        n_protos as u32,
        total_ops as u32,
        total_spans as u32,
        total_lex as u32,
        str_lens.len() as u32,
        str_region.len() as u32,
        interner_lens.len() as u32,
        interner_region.len() as u32,
    ] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    body.extend_from_slice(pod_bytes(&hot));
    for p in &vm.protos {
        body.extend_from_slice(pod_bytes(&p.code));
    }
    for p in &vm.protos {
        body.extend_from_slice(pod_bytes(&p.op_spans));
    }
    for p in &vm.protos {
        body.extend_from_slice(pod_bytes(&p.lexical_scope));
    }
    body.extend_from_slice(pod_bytes(&str_lens));
    body.extend_from_slice(pod_bytes(&interner_lens));
    body.extend_from_slice(&str_region);
    body.extend_from_slice(&interner_region);
    body.extend_from_slice(&cold_bytes);

    let sum = body_checksum(&body);
    let mut bytes = Vec::with_capacity(HEADER_LEN + body.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&key.to_le_bytes());
    bytes.extend_from_slice(&sum.to_le_bytes());
    bytes.extend_from_slice(&body);
    let encode_ns = t_encode.map_or(0, |t| t.elapsed().as_nanos() as u64);

    let t_write = prof.then(std::time::Instant::now);
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Atomic publish: write to a pid-suffixed temp file then
    // rename. Concurrent constructors either see the old blob, the
    // new blob, or no blob — never a torn one.
    let tmp = dir.join(format!(
        "preamble-{key:016x}.tmp.{}",
        std::process::id(),
    ));
    if std::fs::write(&tmp, &bytes).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let _ = std::fs::rename(&tmp, cache_file(dir, key));
    if let Some(t_write) = t_write {
        eprintln!(
            "startup-prof: preamble-cache store blob={}B encode={:.3}ms write={:.3}ms",
            bytes.len(),
            encode_ns as f64 / 1e6,
            t_write.elapsed().as_nanos() as f64 / 1e6,
        );
    }
}

/// The CLI's default cache directory: `$RUBYRS_CACHE_DIR`, else
/// `$XDG_CACHE_HOME/rubyrs`, else `$HOME/.cache/rubyrs`, else
/// `None` (cache disabled). Exposed for the CLI binary; library
/// embedders pass an explicit directory via
/// `Config::preamble_cache_dir` instead.
pub fn default_cache_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("RUBYRS_CACHE_DIR") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(d).join("rubyrs"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache").join("rubyrs"))
}

#[cfg(test)]
mod tests {

    fn test_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "rubyrs-pc-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    fn mk(dir: &std::path::Path) -> crate::Runtime {
        crate::Runtime::with_config(crate::Config {
            preamble_cache_dir: Some(dir.to_path_buf()),
            ..Default::default()
        })
    }

    fn blob_path(dir: &std::path::Path) -> std::path::PathBuf {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "bin"))
            .expect("blob file")
    }

    /// Round-trip the snapshot encoding through a real Vm pair:
    /// store from one freshly-preambled Runtime, load into a
    /// second, and check the second produces identical eval
    /// results. Uses a tempdir so parallel test runs don't share
    /// state.
    #[test]
    fn snapshot_roundtrip_via_runtime() {
        let dir = test_dir("test");
        let _ = std::fs::remove_dir_all(&dir);

        // First construction: cache miss → live compile → store.
        let mut a = mk(&dir);
        assert!(!a.preamble_cache_hit());
        // Second: must hit and behave identically.
        let mut b = mk(&dir);
        assert!(b.preamble_cache_hit(), "second construction should hit the cache");

        let probe = r#"
            class PcProbe
              def initialize(n); @n = n; end
              def go(k: 2); [@n * k, "s-#{@n}".upcase, (1..3).map { |i| i + @n }]; end
            end
            begin
              raise ArgumentError, "boom" if PcProbe.new(3).go.first != 6
              PcProbe.new(4).go(k: 10).inspect
            rescue ArgumentError => e
              "rescued: #{e.message}"
            end
        "#;
        let va = a.eval(probe, "probe.rb").expect("live runtime eval");
        let vb = b.eval(probe, "probe.rb").expect("cached runtime eval");
        assert_eq!(format!("{va:?}"), format!("{vb:?}"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Field-level store/load equivalence: every Proto field decoded
    /// from the blob must equal its live-compiled twin, byte for
    /// byte. Compares `decode_body` output directly against the
    /// storing Vm's table (comparing two RUNTIMES would see replay-
    /// execution noise instead: preamble execution appends a couple
    /// of runtime-synthesized `<primitive-alias-forwarder>` protos,
    /// so a hit table is a strict superset of the stored one — a
    /// pre-existing v4 property, not a decode artifact). Debug
    /// output covers all fields (derived); the runtime-only
    /// `getter_slot` cache Cell is normalized to unfilled on the
    /// live side first (execution may fill it).
    #[test]
    fn snapshot_proto_field_equivalence() {
        let dir = test_dir("fields");
        let _ = std::fs::remove_dir_all(&dir);
        let a = mk(&dir); // miss → live compile + store
        let bytes = std::fs::read(blob_path(&dir)).unwrap();
        let dec = super::decode_body(&bytes[super::HEADER_LEN..]).expect("decode_body");
        assert!(dec.protos.len() > 500, "expected a real preamble proto table");
        // The live table may have grown PAST the store point
        // (post-preamble construction steps); the stored prefix must
        // match exactly.
        assert!(dec.protos.len() <= a.vm.protos.len());
        for (i, (live, decoded)) in a.vm.protos.iter().zip(dec.protos.iter()).enumerate() {
            live.getter_slot.set(u32::MAX);
            assert_eq!(format!("{live:?}"), format!("{decoded:?}"), "proto {i} diverged");
        }
        // Interner: the blob carries the full table at store time —
        // it must be a prefix of the live runtime's (which may have
        // interned more since).
        assert!(dec.interner.len() <= a.vm.interner.len());
        for (i, s) in dec.interner.iter().enumerate() {
            assert_eq!(
                &**a.vm.interner.resolve(crate::intern::SymId(i as u32)),
                *s,
                "sym {i} diverged"
            );
        }
        // And a hit runtime must behave identically end-to-end.
        let mut b = mk(&dir);
        assert!(b.preamble_cache_hit());
        let v = b
            .eval("[:a, :b].map(&:to_s).join('-').size", "fields.rb")
            .expect("hit eval");
        assert_eq!(format!("{v:?}"), "Int(3)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Manual profiling harness: attributes the warm-HIT cost across
    /// the v5 phases (checksum vs decode) over a real blob. Run with
    /// `cargo test --release -p rubyrs --lib decode_split_profile -- --ignored --nocapture`.
    #[test]
    #[ignore = "manual profiling harness, prints a decode-cost split"]
    fn decode_split_profile() {
        let dir = test_dir("split");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = mk(&dir);
        let bytes = std::fs::read(blob_path(&dir)).unwrap();

        fn median_ns(mut f: impl FnMut()) -> u64 {
            let mut samples: Vec<u64> = (0..30)
                .map(|_| {
                    let t = std::time::Instant::now();
                    f();
                    t.elapsed().as_nanos() as u64
                })
                .collect();
            samples.sort_unstable();
            samples[samples.len() / 2]
        }

        let body = &bytes[super::HEADER_LEN..];
        let t_sum = median_ns(|| {
            std::hint::black_box(super::body_checksum(body));
        });
        let t_decode = median_ns(|| {
            std::hint::black_box(super::decode_body(body).unwrap());
        });
        let dec = super::decode_body(body).unwrap();
        let n_ops: usize = dec.protos.iter().map(|p| p.code.len()).sum();
        eprintln!(
            "decode-split(v5): blob={}B protos={} ops={} | checksum={:.3}ms decode={:.3}ms",
            bytes.len(),
            dec.protos.len(),
            n_ops,
            t_sum as f64 / 1e6,
            t_decode as f64 / 1e6,
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt cache file must fall back to live compile, not
    /// panic or mis-restore.
    #[test]
    fn corrupt_blob_falls_back_to_live() {
        let dir = test_dir("corrupt");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = mk(&dir); // populate
        // Truncate / scribble every cache file in the dir.
        for ent in std::fs::read_dir(&dir).unwrap().flatten() {
            std::fs::write(ent.path(), b"RBPCgarbage").unwrap();
        }
        let mut rt = mk(&dir);
        assert!(!rt.preamble_cache_hit());
        let v = rt.eval("[1, 2, 3].sum", "p.rb").expect("eval after fallback");
        assert_eq!(format!("{v:?}"), "Int(6)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Corruption battery: truncations at many points and bit flips
    /// in every section (header, checksum, POD regions, string
    /// regions, postcard tail) must ALL fall back silently to live
    /// compile with correct program output — this is the checksum's
    /// job (the pre-v5 format could panic or, worse, mis-decode).
    #[test]
    fn corruption_battery_falls_back() {
        let dir = test_dir("battery");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = mk(&dir); // populate
        let path = blob_path(&dir);
        let original = std::fs::read(&path).unwrap();
        let n = original.len();
        assert!(n > 4096, "blob unexpectedly small: {n}");

        let mut cases: Vec<(String, Vec<u8>)> = Vec::new();
        // Truncations: inside the header, right after it, and at
        // several points through every section.
        for cut in [3usize, 17, super::HEADER_LEN, n / 8, n / 4, n / 2, 3 * n / 4, n - 1] {
            cases.push((format!("truncate@{cut}"), original[..cut].to_vec()));
        }
        // Bit flips: format-version byte, key, checksum itself, the
        // fixed body header, and points through the POD / string /
        // postcard sections.
        for flip in [4usize, 9, 17, super::HEADER_LEN + 1, n / 8, n / 4, n / 2, 2 * n / 3, 3 * n / 4, n - 10] {
            let mut b = original.clone();
            b[flip] ^= 0x10;
            cases.push((format!("bitflip@{flip}"), b));
        }
        // Extension: trailing garbage changes the checksummed length.
        let mut extended = original.clone();
        extended.extend_from_slice(b"\x00garbage");
        cases.push(("extend".into(), extended));

        for (label, mutated) in cases {
            std::fs::write(&path, &mutated).unwrap();
            let mut rt = mk(&dir);
            assert!(!rt.preamble_cache_hit(), "{label}: corrupt blob must MISS");
            let v = rt
                .eval("(1..4).map { |i| i * i }.sum", "battery.rb")
                .unwrap_or_else(|e| panic!("{label}: eval failed after fallback: {e:?}"));
            assert_eq!(format!("{v:?}"), "Int(30)", "{label}: wrong output after fallback");
            // The failed load must not clobber the blob; restore for
            // the next case anyway to keep cases independent.
            std::fs::write(&path, &original).unwrap();
        }

        // Restored pristine blob still hits.
        let rt = mk(&dir);
        assert!(rt.preamble_cache_hit(), "pristine blob should hit again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stale blob from the previous format version must be
    /// rejected by the version gate (silently — live compile, then
    /// the store overwrites it with a fresh v5 blob that hits).
    #[test]
    fn stale_format_version_rejected() {
        let dir = test_dir("stale");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = mk(&dir); // populate v5
        let path = blob_path(&dir);
        let mut bytes = std::fs::read(&path).unwrap();
        // Rewrite the version field to the previous format's.
        bytes[4..8].copy_from_slice(&(super::FORMAT_VERSION - 1).to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let mut rt = mk(&dir);
        assert!(!rt.preamble_cache_hit(), "v4-tagged blob must MISS");
        let v = rt.eval("'ok'.upcase.length + 5", "stale.rb").expect("eval after fallback");
        assert_eq!(format!("{v:?}"), "Int(7)");
        // The miss path re-stored a valid v5 blob.
        let rt2 = mk(&dir);
        assert!(rt2.preamble_cache_hit(), "re-stored blob should hit");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
