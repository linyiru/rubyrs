//! `_json_native` — serde_json-backed accelerator for the
//! pure-Ruby JSON canon (`src/stdlib_vendor/json.rb`).
//!
//! ADR 0019 Rule 6 partition: the pure canon stays the spec —
//! every observable behaviour (Ruby value shape, generated
//! bytes, error class hierarchy) is whatever the canon produces.
//! This battery is the behaviour-equivalent fast path. The two
//! agree byte-for-byte on the deterministic subset (Null / Bool /
//! Integer / Float / String / Array / Hash); the pure canon's
//! `json_canon` parity fixture stays the parity claim, and this
//! file's correctness reduces to "produces the same Value /
//! emit-bytes the canon would have."
//!
//! Two host fns registered:
//!   - `__rubyrs_json_native_generate(value) → String`
//!     compact-form JSON of `value`, matching `JSON.generate`'s
//!     default (no whitespace).
//!   - `__rubyrs_json_native_parse(json_str) → Value`
//!     serde_json parse + Ruby value reconstruction. Hash keys
//!     are always String (matches canon default; the
//!     `symbolize_names` option stays in the pure canon's
//!     wrapper).
//!
//! The canon's `JSON.parse` / `JSON.generate` Ruby methods
//! `defined?(__rubyrs_json_native_generate)`-detect and prefer
//! the native path when the host fns are registered. Embedders
//! who want the pure canon (deterministic, no serde_json
//! transitive deps) simply don't build with `_json_native`; the
//! Tier-1 default already gates the dep behind the feature.

#![cfg(feature = "_json_native")]

use crate::error::{RubyError, Trap};
use crate::heap::{HashObj, HeapObj};
use crate::value::Value;
use crate::vm::current_vm_ptr;

/// Register the two `__rubyrs_json_native_*` host fns on `rt`.
/// Call once per Runtime that wants the accelerator; the pure-
/// Ruby canon's `JSON` module detects the registration via
/// `defined?(...)` and routes hot calls through the native
/// path. Idempotent — re-registration overwrites.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    rt.register_fn("__rubyrs_json_native_generate", |args| {
        let v = match args {
            [v] => v,
            _ => return Err(Trap {
                err: RubyError::ArgumentError {
                    msg: "__rubyrs_json_native_generate(value)".to_string(),
                },
                backtrace: vec![],
            }),
        };
        let ptr = current_vm_ptr();
        if ptr.is_null() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "json_native: CURRENT_VM_PTR null".to_string(),
                },
                backtrace: vec![],
            });
        }
        let vm = unsafe { &*ptr };
        // Emit into a reusable thread-local scratch, then hand the
        // caller an EXACT-size Vec. `Value::new_str_bytes` MOVES its
        // Vec (capacity included) into the result String, so the old
        // per-call `Vec::with_capacity(4096)` pinned 4 KB behind
        // every small result for its whole lifetime (10k retained
        // 50-byte bodies = 40 MB, not 500 KB). Measured across
        // {} / 100 B / 3.4 KB / 1 MB payloads this shape is within
        // ~15-30 ns/call of the fastest (move-the-4KB-Vec) variant
        // and strictly better on memory:
        //   - scratch reuse: no 4 KB malloc/free per call, no growth
        //     re-allocs after the first big payload;
        //   - `to_vec`: one exact-size alloc + memcpy (≤ 64 KB);
        //   - > 64 KB results: move the scratch out instead of
        //     copying (leaves a fresh 4 KB scratch) so huge bodies
        //     stay one-pass.
        // Re-entrancy: `write_value` never runs Ruby code (it
        // declines non-primitive values), but the RefCell keeps this
        // panic-shaped rather than UB if that ever changes.
        thread_local! {
            static SCRATCH: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(Vec::with_capacity(4096));
        }
        SCRATCH.with(|cell| {
            let mut out = cell.borrow_mut();
            out.clear();
            // DECLINE-AS-NIL contract: every write_value error is a
            // decline-to-canon condition (NaN/Infinity, unsupported
            // values, non-UTF-8 strings, >100 nesting) — return nil
            // instead of a Trap so the Ruby wrapper needs no
            // begin/rescue on the hot path (a rescue frame costs
            // ~33 ns/call) and declines skip Trap construction
            // entirely. nil is unambiguous: JSON.generate otherwise
            // always produces a String. The arity/VM-null checks
            // above remain real Traps.
            if write_value(vm, v, &mut out, 0).is_err() {
                return Ok(Value::Nil);
            }
            if out.len() > 65536 {
                let big = std::mem::replace(&mut *out, Vec::with_capacity(4096));
                Ok(Value::new_str_bytes(big))
            } else {
                Ok(Value::new_str_bytes(out.as_slice().to_vec()))
            }
        })
    });

    rt.register_fn("__rubyrs_json_native_parse", |args| {
        let s = match args {
            [Value::Str(s)] => s,
            _ => return Err(Trap {
                err: RubyError::ArgumentError {
                    msg: "__rubyrs_json_native_parse(json_str: String)".to_string(),
                },
                backtrace: vec![],
            }),
        };
        // serde_json requires UTF-8; CRuby's parser passes raw
        // bytes in string values through untouched. Decline
        // non-UTF-8 input to the pure canon (the wrapper matches
        // on "non-utf8") instead of silently U+FFFD-mangling it
        // (which the old `to_string_lossy` copy did). The check
        // is O(n) once and cached on the RStr (`utf8_cache`), so
        // repeated parses of the same buffer don't rescan.
        if !s.content.is_utf8_cached() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "json_native: non-utf8 input".to_string(),
                },
                backtrace: vec![],
            });
        }
        // Exponent-quirk fence: CRuby's parser has two overflow
        // shortcuts serde can't reproduce from the PARSED f64 alone
        // (the value loses the written digit count / adjusted-exp
        // information): exponent LITERALS of ≥20 digits (or value
        // past i64) saturate, and an adjusted exponent past
        // INT32_MAX overflows to ±Infinity even for a ZERO mantissa
        // ("0.0e2147483649" → Infinity). Both need a written
        // exponent of ≥10 digits, so any document containing a
        // number-context `[eE][+-]?` followed by a ≥10-digit run
        // declines whole to the pure canon, which implements the
        // exact rules (single value+error authority, pathological
        // literals only — real payloads never carry 10-digit
        // exponents). Everything BELOW that fence (`1e999`,
        // `1e-999999999`, …) agrees between Rust's correctly-
        // rounded f64 parse and CRuby's, so no rule porting is
        // needed on the fast path.
        if has_exp_out_of_range(&s.content.borrow()) {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "json_native: exponent-out-of-range literal".to_string(),
                },
                backtrace: vec![],
            });
        }
        // Direct-visitor parse: skip the `serde_json::Value`
        // intermediate tree (the obvious-but-slow shape that
        // allocates twice — once into Rust, once into Ruby).
        // The visitor calls `vm.heap.alloc` for Array / Hash
        // during the serde state walk, so a 3.4 KB JSON payload
        // pays one full allocation pass instead of two.
        let ptr = current_vm_ptr();
        if ptr.is_null() {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "json_native: CURRENT_VM_PTR null — called outside host-fn scope".to_string(),
                },
                backtrace: vec![],
            });
        }
        // SAFETY: ptr is set by the dispatch site immediately
        // before this closure runs; each `&mut` re-derived from it
        // below lasts only for one synchronous deserialize pass and
        // isn't stashed anywhere (the passes run strictly one after
        // the other, never overlapping).
        //
        // GC safe point: the interpreter's maybe_gc only lives at
        // ITS alloc sites, and a `loop { JSON.parse(s) }` allocs
        // almost nothing interpreter-side — the parse trees this
        // host fn allocates would pile up unbounded (measured:
        // 1000 discarded 830 KB parses grew RSS into the GBs).
        // Right here is safe: no Values are held by this call yet
        // (the input is an Rc-managed Str, not a GC-heap object),
        // and everything else live is rooted from the VM stack.
        // The generate host fn deliberately does NOT do this — its
        // argument Value may only be rooted by the caller's frame,
        // and it allocates no GC-heap objects anyway.
        unsafe { &mut *ptr }.maybe_gc();
        // Borrow the input bytes directly (no copy). The `Ref`
        // lives across the visitor's `&mut vm` use — that's fine:
        // the RStr is Rc-owned by the caller's argument slot (not
        // a GC-heap object), no Ruby code runs inside the visitor,
        // and `Heap::alloc` never collects, so nothing can mutate
        // or free the buffer mid-parse.
        let bytes = s.content.borrow();
        let map_parse_err = |e: serde_json::Error| {
            // The visitor's own depth guard raises "nesting of N is
            // too deep"; serde wraps custom messages with a
            // " at line X column Y" suffix and we'd prefix
            // "native parse: " — both would leak into the
            // JSON::NestingError message the wrapper re-raises.
            // Strip back to the exact CRuby text for that case.
            let msg = e.to_string();
            let msg = match (msg.find("nesting of"), msg.find(" is too deep")) {
                (Some(start), Some(end)) => msg[start..end + " is too deep".len()].to_string(),
                _ => format!("native parse: {msg}"),
            };
            Trap {
                err: RubyError::RuntimeError { msg },
                backtrace: vec![],
            }
        };
        // Pass 1 — the always-taken fast path. Numbers arrive
        // through serde's native i64/u64/f64 lanes at full speed;
        // the only additions over plain serde are (a) u64 values
        // past i64::MAX becoming exact Bignums (CRuby Integer
        // semantics — serde parses 19–20-digit ints exactly as
        // u64) and (b) a three-compare suspicion check on every
        // f64 (see `f64_needs_exact`) that flags values which MAY
        // have lost integer precision or negative-zero identity
        // inside serde. A flagged value aborts pass 1 and requests
        // the exact retry below instead of declining to the
        // ~200×-slower interpreted canon.
        let (first, retry) = KEY_CACHE.with(|kc| {
            let mut kc = kc.borrow_mut();
            let mut ctx = ParseCtx {
                vm: unsafe { &mut *ptr },
                depth: 0,
                keys: &mut kc,
                exact: None,
                retry: false,
            };
            let mut de = serde_json::Deserializer::from_slice(&bytes);
            let result = serde::de::Deserializer::deserialize_any(&mut de, VmVisitor { ctx: &mut ctx })
                .and_then(|v| de.end().map(|()| v));
            (result, ctx.retry)
        });
        match first {
            Ok(v) => Ok(v),
            Err(e) if !retry => Err(map_parse_err(e)),
            Err(_) => {
                // Pass 2 — exact-number retry. A string-aware scan
                // extracts every number-context literal that needs
                // exact treatment (ints beyond ±u64/i64 range,
                // negative zeros, huge floats) IN DOCUMENT ORDER,
                // then the same serde parse re-runs consuming that
                // queue: each suspicious f64 visit must pair with
                // the queue head (bit-identical expected f64) and
                // is replaced by the exact value re-derived from
                // the literal's raw text. Any disagreement between
                // the scan and serde's tokenization (malformed
                // docs, scanner gaps) breaks the pairing and
                // declines to the canon — wrong values cannot
                // escape, only speed. Partially-built pass-1
                // containers are unrooted garbage; the next GC
                // safe point collects them.
                let queue = scan_exact_literals(&bytes);
                KEY_CACHE.with(|kc| {
                    let mut kc = kc.borrow_mut();
                    let mut ctx = ParseCtx {
                        vm: unsafe { &mut *ptr },
                        depth: 0,
                        keys: &mut kc,
                        exact: Some(ExactQueue { entries: queue, pos: 0 }),
                        retry: false,
                    };
                    let mut de = serde_json::Deserializer::from_slice(&bytes);
                    let result = serde::de::Deserializer::deserialize_any(
                        &mut de,
                        VmVisitor { ctx: &mut ctx },
                    )
                    .and_then(|v| de.end().map(|()| v))
                    .map_err(map_parse_err)?;
                    // Every queued literal must have been consumed —
                    // leftovers mean the scan saw numbers serde
                    // didn't (tokenization disagreement): decline.
                    let q = ctx.exact.as_ref().expect("exact queue present on pass 2");
                    if q.pos != q.entries.len() {
                        return Err(Trap {
                            err: RubyError::RuntimeError {
                                msg: "json_native: exact-number pairing miss".to_string(),
                            },
                            backtrace: vec![],
                        });
                    }
                    Ok(result)
                })
            }
        }
    });
}

/// Recursive byte-buffer serializer. `out` is a `Vec<u8>` so
/// strings copy via `extend_from_slice` (memcpy) instead of
/// going through `String`'s UTF-8 invariant check on each
/// push. Mirrors the pure canon's `generate_with` emit shape
/// byte-for-byte (the parity contract the json_canon fixture
/// pins).
fn write_value(vm: &crate::vm::Vm, v: &Value, out: &mut Vec<u8>, depth: u32) -> Result<(), Trap> {
    // CRuby's generator allows container depth ≤ max_nesting (100
    // by default) and raises JSON::NestingError past it. Decline to
    // the canon (which raises the exact CRuby class + message) —
    // this also bounds our own native recursion on hostile inputs.
    const MAX_GEN_NESTING: u32 = 100;
    match v {
        Value::Nil => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Int(n) => write_int(*n, out),
        #[cfg(feature = "bignum")]
        Value::BigInt(id) => {
            // CRuby's generator emits Bignum via to_s (bare decimal
            // digits, no quotes) — `generate_json_bignum`.
            out.extend_from_slice(vm.heap.bigint(*id).to_string().as_bytes());
        }
        Value::Float(f) => {
            if f.is_nan() || f.is_infinite() {
                return Err(Trap {
                    err: RubyError::RuntimeError {
                        msg: format!("{f} not allowed in JSON"),
                    },
                    backtrace: vec![],
                });
            }
            // CRuby's json gem does NOT emit Float#to_s — it runs
            // fpconv (Grisu2 + fpconv's fixed/scientific window
            // rule: `1e15` → "1e+15", `1.5e-5` → "0.000015"). The
            // exact port in json_float.rs is also ~4× faster than
            // the old `write!(out, "{:.1}", f)` shape (no fmt
            // machinery, no precision pass).
            crate::json_float::write_json_float(*f, out);
        }
        Value::Str(s) => {
            if str_needs_canon(s) {
                return Err(str_decline());
            }
            let b = s.content.borrow();
            write_escaped_bytes(&b, out);
        }
        Value::Sym(id) => {
            let rc = vm.interner.resolve(*id);
            write_escaped_bytes(rc.as_bytes(), out);
        }
        Value::Array(id) => {
            // No clone: `vm.heap.array` returns `&Vec<Value>`
            // borrowed from `&Vm`, and recursive `write_value`
            // calls also take `&Vm` — multiple immutable
            // borrows coexist. Skipping the clone saves one
            // Value-vec allocation per Array node (~100 entries
            // on the bench payload's outer Object value list,
            // 20 on each nested Hash's "tags" array).
            if depth + 1 > MAX_GEN_NESTING {
                return Err(Trap {
                    err: RubyError::RuntimeError {
                        msg: "json_native: nesting too deep".to_string(),
                    },
                    backtrace: vec![],
                });
            }
            let items = vm.heap.array(*id);
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 { out.push(b','); }
                // Inline the scalar arms: the recursive call isn't
                // inlinable (self-recursion) and containers are
                // mostly scalars — skipping the call for
                // Int/Nil/Bool measurably trims the hot loop.
                match item {
                    Value::Int(n) => write_int(*n, out),
                    Value::Nil => out.extend_from_slice(b"null"),
                    Value::Bool(b) => {
                        out.extend_from_slice(if *b { b"true" } else { b"false" })
                    }
                    other => write_value(vm, other, out, depth + 1)?,
                }
            }
            out.push(b']');
        }
        Value::Hash(id) => {
            if depth + 1 > MAX_GEN_NESTING {
                return Err(Trap {
                    err: RubyError::RuntimeError {
                        msg: "json_native: nesting too deep".to_string(),
                    },
                    backtrace: vec![],
                });
            }
            let pairs = vm.heap.hash(*id);
            out.push(b'{');
            for (i, (k, val)) in pairs.iter().enumerate() {
                if i > 0 { out.push(b','); }
                // CRuby JSON.generate stringifies non-String keys
                // via to_s — Symbol → name, Integer → decimal repr,
                // Float → Float#to_s (NOT the fpconv value form:
                // `{1e-5 => 1}` emits `{"1.0e-05":1}` on CRuby),
                // nil → "", true/false → their names. Anything else
                // (Array / Hash / Object keys) declines to the pure
                // canon, whose `k.to_s` handles the long tail.
                match k {
                    Value::Str(s) => {
                        if str_needs_canon(s) {
                            return Err(str_decline());
                        }
                        let b = s.content.borrow();
                        write_escaped_bytes(&b, out);
                    }
                    Value::Sym(sid) => {
                        let rc = vm.interner.resolve(*sid);
                        write_escaped_bytes(rc.as_bytes(), out);
                    }
                    Value::Int(n) => {
                        out.push(b'"');
                        write_int(*n, out);
                        out.push(b'"');
                    }
                    Value::Float(f) => {
                        // Hash keys go through to_s on CRuby, and
                        // Float#to_s NaN/Infinity are legal STRING
                        // keys (`{"NaN":1}`) — no finiteness raise.
                        let s = crate::heap::format_float(*f);
                        write_escaped_bytes(s.as_bytes(), out);
                    }
                    #[cfg(feature = "bignum")]
                    Value::BigInt(id) => {
                        out.push(b'"');
                        out.extend_from_slice(vm.heap.bigint(*id).to_string().as_bytes());
                        out.push(b'"');
                    }
                    Value::Nil => out.extend_from_slice(b"\"\""),
                    Value::Bool(b) => {
                        out.extend_from_slice(if *b { b"\"true\"" } else { b"\"false\"" });
                    }
                    other => {
                        return Err(Trap {
                            err: RubyError::RuntimeError {
                                msg: format!("json_native: unsupported key {other:?}"),
                            },
                            backtrace: vec![],
                        });
                    }
                }
                out.push(b':');
                match val {
                    Value::Int(n) => write_int(*n, out),
                    Value::Nil => out.extend_from_slice(b"null"),
                    Value::Bool(b) => {
                        out.extend_from_slice(if *b { b"true" } else { b"false" })
                    }
                    other => write_value(vm, other, out, depth + 1)?,
                }
            }
            out.push(b'}');
        }
        other => {
            // Anything outside the deterministic subset bails
            // back to the pure canon by returning Trap — the
            // canon's wrapper rescues and re-runs the value via
            // its case/when dispatch (which knows about Object
            // fall-through, etc.).
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: format!("json_native: unsupported value {:?}", other),
                },
                backtrace: vec![],
            });
        }
    }
    Ok(())
}

/// Strings CRuby's generator wouldn't emit verbatim: BINARY
/// (ASCII-8BIT) with non-ASCII content — CRuby warns-and-emits
/// when the bytes happen to be valid UTF-8, raises
/// `JSON::GeneratorError` ('"\xNN" from ASCII-8BIT to UTF-8')
/// otherwise — and any string whose content is malformed UTF-8
/// ("source sequence is illegal/malformed utf-8"). Both decline
/// to the pure canon, whose `escape_string` reproduces the exact
/// CRuby class + message. The checks are O(1) after the RStr's
/// ascii/utf8 caches warm (parse-produced strings are pre-marked
/// valid at construction).
fn str_needs_canon(s: &crate::value::RStr) -> bool {
    match s.encoding.get() {
        crate::value::EncodingTag::Binary => !s.content.is_ascii_cached(),
        _ => !s.content.is_utf8_cached(),
    }
}

fn str_decline() -> Trap {
    Trap {
        err: RubyError::RuntimeError {
            msg: "json_native: string needs canon encoding handling".to_string(),
        },
        backtrace: vec![],
    }
}

/// Written-exponent length past which a number literal enters
/// CRuby's overflow-shortcut regimes (adjusted exponent >
/// INT32_MAX and, further out, the ≥20-digit/±i64 literal
/// saturation) — behaviours that can't be recovered from the
/// PARSED f64. Documents carrying one decline whole to the canon.
/// A ≥10-digit exponent is the shortest that can push the
/// adjusted exponent past INT32_MAX (2147483648 is 10 digits).
const EXP_FENCE_DIGITS: usize = 10;

/// True when `bytes` contains, in NUMBER context (outside JSON
/// string literals), an exponent marker `e`/`E` followed by an
/// optional sign and ≥[`EXP_FENCE_DIGITS`] digits. Two tiers:
///
///   1. Cheap filter: a strided scan (any ≥10-digit run contains
///      a position probed by a stride-10 walk) finds digit runs,
///      expands LEFT to the run start, and checks the 1–2 bytes
///      before it for an exponent prefix. Benign payloads (short
///      digit runs, or long runs preceded by quotes/colons — the
///      snowflake-ID-in-string shape) cost ~n/10 probes plus a
///      few bytes per digit hit and never reach tier 2. This
///      replaced a 19-digit BIGINT pre-scan whose tier 2 walked
///      the entire document on every ID-heavy payload (~3-4 µs on
///      an 11 KB doc); bigint-range integers now parse natively
///      via the exact-retry pass instead of declining.
///   2. Precise pass, only on a tier-1 candidate (or a >40-digit
///      run, where left-expansion is capped): a byte state
///      machine tracks in-string state (escape-aware) and looks
///      for the exponent shape outside strings.
///
/// False positives (an e-prefixed ≥10-digit run outside a string
/// in a MALFORMED document) still just decline to the canon,
/// whose strict number grammar raises like CRuby.
fn has_exp_out_of_range(bytes: &[u8]) -> bool {
    const STRIDE: usize = EXP_FENCE_DIGITS;
    // Left-expansion cap: probes landing deep inside very long
    // digit runs (>40 digits) hand the whole document to the
    // precise tier instead of re-walking the run per probe
    // (keeps the filter linear on digit-blob inputs).
    const EXPAND_CAP: usize = 40;
    let n = bytes.len();
    let mut i = STRIDE - 1;
    while i < n {
        if !bytes[i].is_ascii_digit() {
            i += STRIDE;
            continue;
        }
        // Expand left to the run start.
        let mut lo = i;
        let mut steps = 0usize;
        loop {
            if lo == 0 {
                break;
            }
            if !bytes[lo - 1].is_ascii_digit() {
                break;
            }
            lo -= 1;
            steps += 1;
            if steps > EXPAND_CAP {
                return has_exp_out_of_range_precise(bytes);
            }
        }
        // Exponent prefix directly before the run?
        let prefixed = (lo >= 1 && matches!(bytes[lo - 1], b'e' | b'E'))
            || (lo >= 2
                && matches!(bytes[lo - 1], b'+' | b'-')
                && matches!(bytes[lo - 2], b'e' | b'E'));
        if prefixed {
            // Candidate: measure the run; ≥ fence → precise pass.
            let mut hi = i + 1;
            while hi < n && bytes[hi].is_ascii_digit() {
                hi += 1;
            }
            if hi - lo >= EXP_FENCE_DIGITS {
                return has_exp_out_of_range_precise(bytes);
            }
            i = hi + STRIDE;
        } else {
            i += STRIDE;
        }
    }
    false
}

/// Tier 2 of [`has_exp_out_of_range`]: string-aware (escape-
/// tracking) search for `[eE][+-]?\d{10,}` outside string
/// literals. Runs only on tier-1 candidates — essentially never
/// on real payloads.
fn has_exp_out_of_range_precise(bytes: &[u8]) -> bool {
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < n {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'e' | b'E' => {
                let mut j = i + 1;
                if j < n && matches!(bytes[j], b'+' | b'-') {
                    j += 1;
                }
                let digits_start = j;
                while j < n && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j - digits_start >= EXP_FENCE_DIGITS {
                    return true;
                }
                i = j.max(i + 1);
            }
            _ => i += 1,
        }
    }
    false
}

/// The f64 magnitudes at which a value arriving at `visit_f64`
/// may be a silently-rounded INTEGER literal rather than a float
/// literal: serde parses positive integers ≤ u64::MAX and
/// negative integers ≥ i64::MIN exactly (visit_u64/visit_i64);
/// everything beyond falls into f64 with these least magnitudes.
const F64_SUSPECT_POS: f64 = 18_446_744_073_709_551_616.0; // 2^64
const F64_SUSPECT_NEG: f64 = -9_223_372_036_854_775_808.0; // i64::MIN
const NEG_ZERO_BITS: u64 = 0x8000_0000_0000_0000;

/// True when an f64 arriving at `visit_f64` cannot be trusted as
/// the CRuby parse result on its own:
///   - |n| at/past the integer-precision-loss thresholds — the
///     literal may have been an exact bigint (CRuby: Integer);
///   - negative zero — the literal may have been `-0` (CRuby:
///     Integer 0) or a float spelling (CRuby: Float -0.0).
/// Three predictable compares; the fast-path cost replaces the
/// old `n == 0.0 && n.is_sign_negative()` negative-zero decline.
#[inline]
fn f64_needs_exact(n: f64) -> bool {
    n >= F64_SUSPECT_POS || n <= F64_SUSPECT_NEG || n.to_bits() == NEG_ZERO_BITS
}

/// One exact-retry queue entry: the f64 serde is expected to
/// deliver for this literal (bit pattern) + how to rebuild the
/// exact Ruby value from the raw text.
enum Exact {
    /// `-0` int spelling → Integer 0 (CRuby semantics).
    Int0,
    /// Float literal that trips the suspicion thresholds (huge
    /// magnitude or float-spelled negative zero) → keep the f64.
    Float,
    /// Integer literal beyond ±u64/i64 range → exact Bignum from
    /// the stored decimal text.
    Big(Box<[u8]>),
}

struct ExactQueue {
    entries: Vec<(u64, Exact)>,
    pos: usize,
}

/// String-aware scan extracting, IN DOCUMENT ORDER, every number-
/// context literal that needs exact treatment (see [`Exact`]).
/// Runs only on the exact-retry pass. The scan is deliberately
/// loose (maximal-munch over number bytes, no grammar check): a
/// literal span either matches serde's tokenization exactly —
/// JSON numbers are self-delimiting, and every byte this scan
/// over-consumes would also make serde error out — or the retry
/// pass breaks pairing / errors and declines to the canon. Wrong
/// values cannot escape; disagreement only costs speed.
fn scan_exact_literals(bytes: &[u8]) -> Vec<(u64, Exact)> {
    let mut out = Vec::new();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < n {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                i += 1;
                while i < n
                    && matches!(bytes[i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    i += 1;
                }
                classify_exact_literal(&bytes[start..i], &mut out);
            }
            _ => i += 1,
        }
    }
    out
}

/// Queue-entry classification for one number-literal span. Skips
/// literals the fast lanes already handle exactly (i64/u64-range
/// ints, ordinary floats); junk spans (malformed docs) are skipped
/// too — serde errors on them before pairing matters.
fn classify_exact_literal(text: &[u8], out: &mut Vec<(u64, Exact)>) {
    let is_float = text
        .iter()
        .any(|b| matches!(b, b'.' | b'e' | b'E'));
    if is_float {
        // Rust's from_str is correctly rounded (same shortest-
        // round-trip semantics as serde's float_roundtrip parse
        // and CRuby's strtod), so the expected-f64 pairing bits
        // match serde's delivery exactly. Exponent-overflow
        // quirk literals never reach here — the ≥10-digit
        // exponent fence declined those documents up front.
        let Ok(s) = std::str::from_utf8(text) else { return };
        let Ok(f) = s.parse::<f64>() else { return };
        if f64_needs_exact(f) {
            out.push((f.to_bits(), Exact::Float));
        }
        return;
    }
    let neg = text[0] == b'-';
    let digits: &[u8] = if neg { &text[1..] } else { text };
    if digits.is_empty() {
        return; // bare "-": serde errors first
    }
    if neg && digits.iter().all(|&b| b == b'0') {
        // "-0": serde delivers f64 -0.0; CRuby parses Integer 0.
        // (Invalid spellings like "-00" also land here — serde's
        // leading-zero grammar error declines those docs first.)
        out.push((NEG_ZERO_BITS, Exact::Int0));
        return;
    }
    if int_fits_native(neg, digits) {
        return; // arrives via visit_i64/visit_u64, exact already
    }
    let Ok(s) = std::str::from_utf8(text) else { return };
    let Ok(f) = s.parse::<f64>() else { return };
    out.push((f.to_bits(), Exact::Big(text.into())));
}

/// Whether an integer literal (sign stripped, `digits` nonempty)
/// arrives exactly through serde's native lanes: negatives fit
/// i64 (visit_i64), positives fit u64 (visit_u64 — the >i64::MAX
/// span becomes an exact Bignum from the u64 value). Compares as
/// strings to stay allocation-free; leading zeros are invalid
/// JSON and error out in serde regardless of the answer here.
fn int_fits_native(neg: bool, digits: &[u8]) -> bool {
    let limit: &[u8] = if neg {
        b"9223372036854775808" // -i64::MIN
    } else {
        b"18446744073709551615" // u64::MAX
    };
    match digits.len().cmp(&limit.len()) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => digits <= limit,
    }
}

/// Cold arm of `visit_u64`: exact Bignum for the (i64::MAX,
/// u64::MAX] span. Outlined so the ubiquitous small-int lane
/// stays under the inline threshold (see the visit_u64 comment).
#[cold]
#[inline(never)]
fn visit_u64_bignum<E: serde::de::Error>(
    _ctx: &mut ParseCtx<'_>,
    _n: u64,
) -> Result<Value, E> {
    #[cfg(feature = "bignum")]
    {
        let id = _ctx
            .vm
            .heap
            .alloc(HeapObj::BigInt(num_bigint::BigInt::from(_n)));
        Ok(Value::BigInt(id))
    }
    #[cfg(not(feature = "bignum"))]
    {
        Err(serde::de::Error::custom("bigint-range number"))
    }
}

/// Exact Integer/Bignum from a (possibly signed) decimal literal.
/// Fast path: fits i128 → fold natively; else num-bigint parse.
#[cfg(feature = "bignum")]
fn bigint_value_from_text(vm: &mut crate::vm::Vm, text: &[u8]) -> Option<Value> {
    let b = num_bigint::BigInt::parse_bytes(text, 10)?;
    // By construction the caller only passes literals outside
    // i64 range, but normalize defensively — a Value::BigInt
    // holding an i64-range value would break Integer identity
    // assumptions elsewhere.
    if let Ok(n) = i64::try_from(&b) {
        return Some(Value::Int(n));
    }
    Some(Value::BigInt(vm.heap.alloc(HeapObj::BigInt(b))))
}

/// Write a signed integer as ASCII decimal directly into `out`.
/// Hand-rolled because Rust's `write!(out, "{}")` goes through
/// `fmt::Write` machinery + a Formatter (lazy `pad`, fill, etc.)
/// — ~3× slower than this for the common case. Buffer is 20
/// chars max for i64 (`-9223372036854775808` is 20 chars).
fn write_int(mut n: i64, out: &mut Vec<u8>) {
    if n == 0 {
        out.push(b'0');
        return;
    }
    let negative = n < 0;
    // Wrap-handle MIN — its abs() overflows i64. Convert through
    // u64 to dodge.
    let mut u = if negative { (n as i128).unsigned_abs() as u64 } else { n as u64 };
    // Suppress "unused assignment after negation" warning — `n` was
    // only needed for the negative-test above; drop the local.
    n = 0;
    let _ = n;
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while u > 0 {
        i -= 1;
        buf[i] = b'0' + (u % 10) as u8;
        u /= 10;
    }
    if negative {
        out.push(b'-');
    }
    out.extend_from_slice(&buf[i..]);
}

/// JSON string escape over raw bytes. Two-mode body:
///   - Fast path: scan for a run of "safe" bytes (>= 0x20,
///     != `"`, != `\`) and bulk-copy with extend_from_slice
///     (single memcpy per run).
///   - Slow path: when a byte needs escaping, emit the escape
///     literal then resume scanning.
///
/// ~5× faster than the char-by-char `s.chars().for_each` shape
/// because ASCII runs (the common case in JSON payloads) skip
/// per-byte branches AND skip UTF-8 decoding entirely.
fn write_escaped_bytes(s: &[u8], out: &mut Vec<u8>) {
    out.push(b'"');
    let n = s.len();
    let mut i = 0;
    while i < n {
        // Find end of safe run.
        let run_start = i;
        while i < n {
            let b = s[i];
            if b < 0x20 || b == b'"' || b == b'\\' {
                break;
            }
            i += 1;
        }
        if i > run_start {
            out.extend_from_slice(&s[run_start..i]);
        }
        if i >= n {
            break;
        }
        // Escape one byte and continue.
        let b = s[i];
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x0C => out.extend_from_slice(b"\\f"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            _ => {
                // < 0x20 control char — emit `\u00XX`.
                out.extend_from_slice(b"\\u00");
                const HEX: &[u8; 16] = b"0123456789abcdef";
                out.push(HEX[((b >> 4) & 0x0f) as usize]);
                out.push(HEX[(b & 0x0f) as usize]);
            }
        }
        i += 1;
    }
    out.push(b'"');
}

/// Per-parse state threaded through the visitor recursion.
///
/// GC safety: the key cache holds `Value::Str`s (Rc-managed — NOT
/// GC-heap objects; see `Value::is_gc_heap_ref`) across the
/// visitor's `vm.heap.alloc` calls, so a collection could never
/// free them even if one ran; and the `pairs` / `elems` Vecs that
/// DO hold heap `ObjId`s (Array / Hash / BigInt alike) are safe
/// for the same reason they always were — `Heap::alloc` never
/// collects (collections only run at interpreter safepoints, and
/// no Ruby code runs inside the visitor).
struct ParseCtx<'a> {
    vm: &'a mut crate::vm::Vm,
    /// Container nesting depth. CRuby's parser (json 2.20,
    /// probed) checks nesting when entering a NON-EMPTY container:
    /// 101 nested EMPTY arrays parse fine, while 101 nested arrays
    /// around any element raise `JSON::NestingError` ("nesting of
    /// 101 is too deep" — the first violation is always at depth
    /// max+1, so that is the reported number even for a 150-deep
    /// document). Equivalent formulation used here: raise when
    /// parsing any VALUE nested inside more than `max_nesting`
    /// containers — a value only gets parsed if its container is
    /// non-empty (see `VmSeed::deserialize`). serde_json's own
    /// recursion limit (128) is far enough out that without this
    /// guard, over-deep documents would silently parse (or produce
    /// the wrong error text past 128).
    depth: u32,
    keys: &'a mut KeyCache,
    /// `None` on the primary pass; `Some` on the exact-number
    /// retry pass, holding the document-ordered literal queue
    /// every suspicious `visit_f64` must pair against.
    exact: Option<ExactQueue>,
    /// Set by the primary pass when a suspicious f64 arrived —
    /// the host fn re-parses with the exact queue instead of
    /// declining to the interpreted canon.
    retry: bool,
}

impl ParseCtx<'_> {
    /// Handle a `visit_f64` value flagged by [`f64_needs_exact`].
    /// Primary pass: request the exact retry. Retry pass: pair
    /// against the queue head (bit-identical expectation) and
    /// rebuild the exact value; any mismatch is a scan/serde
    /// tokenization disagreement → decline to the canon.
    /// Outlined + cold: keeps `visit_f64`'s ordinary-float lane
    /// tiny (suspicious values are rare across all parses).
    #[cold]
    #[inline(never)]
    fn exact_number(&mut self, n: f64) -> Result<Value, &'static str> {
        let Some(q) = self.exact.as_mut() else {
            self.retry = true;
            return Err("exact-number retry");
        };
        // Extract an owned action first: the `Big` text must
        // outlive the queue borrow because rebuilding it needs
        // `&mut self.vm` (rare path; the clone is a short digit
        // string).
        enum Act {
            Int0,
            Float,
            Big(Box<[u8]>),
        }
        let act = match q.entries.get(q.pos) {
            Some((bits, _)) if *bits != n.to_bits() => {
                return Err("exact-number pairing miss");
            }
            Some((_, Exact::Int0)) => Act::Int0,
            Some((_, Exact::Float)) => Act::Float,
            Some((_, Exact::Big(text))) => Act::Big(text.clone()),
            None => return Err("exact-number pairing miss"),
        };
        q.pos += 1;
        match act {
            Act::Int0 => Ok(Value::Int(0)),
            Act::Float => Ok(Value::Float(n)),
            Act::Big(_text) => {
                #[cfg(feature = "bignum")]
                {
                    return bigint_value_from_text(self.vm, &_text)
                        .ok_or("exact-number bigint parse");
                }
                #[cfg(not(feature = "bignum"))]
                {
                    // No Bignum representation in this build — the
                    // canon (whose Integer also can't promote here)
                    // stays the authority for the shape.
                    Err("bigint-range number")
                }
            }
        }
    }
}

/// fstring-equivalent object-key cache: fxhash(key bytes) → the
/// FROZEN shared key `Value`. CRuby's parser interns object keys
/// in the process-global fstring table (frozen; duplicate keys
/// are `.equal?` even ACROSS separate `JSON.parse` calls), so
/// sharing one Rc'd frozen Str is not just an allocation win —
/// it's the observable CRuby shape.
///
/// Lifetime: thread-local, persistent across parse calls — a
/// per-parse table was measured first and REJECTED: building it
/// cost ~40 ns/key (hash-map insert + growth rehashing), which
/// regressed unique-key-heavy parses by ~2× (keys_unique fixture
/// 39 → 85 µs/iter) for wins only on repeat-heavy ones. The
/// persistent table amortises inserts away entirely: steady-state
/// parses only pay hash + byte-compare + Rc clone (~10 ns/key)
/// instead of String + Rc allocation (~40 ns + later free). This
/// also matches CRuby MORE closely (its fstring table is
/// process-global). Sharing Rc<RStr> across Vms on one thread is
/// sound: RStr is Rc-managed (no ObjId, no interner id) and the
/// entries are frozen.
///
/// Memory: capped at `KEY_CACHE_CAP` distinct texts (identifier-
/// sized keys ≈ ≤0.5 MB worst case). Past the cap, new key texts
/// stop being cached ([`KeyHit::Uncached`]) — parses stay correct
/// (content-scan dup detection), just without sharing for the
/// overflow keys.
///
/// fxhash is not collision-free, so hits verify bytes and true
/// collisions escalate to a `Many` bucket — adversarial inputs
/// get the right answer, just marginally slower.
#[derive(Default)]
struct KeyCache {
    map: crate::intern::FxHashMap<u64, KeyEntry>,
    /// Count of distinct key texts stored (cap enforcement —
    /// `map.len()` undercounts when `Many` buckets hold several).
    len: usize,
    /// Monotonically increasing visit_map generation — the
    /// duplicate-key detector's clock (see [`KeyState`]).
    obj_gen: u64,
}

const KEY_CACHE_CAP: usize = 8192;

struct KeyState {
    val: Value,
    /// Generation of the object this key text last appeared in +
    /// the pairs-index it landed at. `visit_map` gets a fresh
    /// generation on entry, so:
    ///   - `last_obj == current` → duplicate key within THIS
    ///     object → last-wins overwrite at `last_idx`, O(1);
    ///   - `last_obj < current` → last seen before this object
    ///     opened (a sibling / earlier parse) → plain push, O(1);
    ///   - `last_obj > current` → seen in a DESCENDANT opened
    ///     after this object (`{"a":1,"c":{"a":5},"a":9}`) —
    ///     ambiguous, resolve by pointer-scanning the current
    ///     object's pairs (rare shape; stays correct).
    last_obj: u64,
    last_idx: u32,
}

enum KeyEntry {
    One(KeyState),
    Many(Vec<KeyState>),
}

/// What the key seed hands `visit_map`.
enum KeyHit {
    /// Key is not a duplicate within the current object — push.
    Push(Value),
    /// Duplicate within the current object at this pairs-index —
    /// CRuby is last-wins with the key keeping its position.
    Dup(u32),
    /// Cached key last seen in a descendant object — dup-ness
    /// unknown; resolve by Rc-pointer scan of the current pairs.
    Ambiguous(Value),
    /// Cache at capacity, key text not cached — resolve dup-ness
    /// by content scan of the current pairs.
    Uncached(Value),
}

thread_local! {
    static KEY_CACHE: std::cell::RefCell<KeyCache> = std::cell::RefCell::new(KeyCache::default());
}

/// CRuby's parse-side nesting default (json 2.x `max_nesting`).
const MAX_PARSE_NESTING: u32 = 100;

impl KeyCache {
    fn next_obj_gen(&mut self) -> u64 {
        self.obj_gen += 1;
        self.obj_gen
    }

    /// Look up (or create) the shared frozen key Value for `s`,
    /// classifying its duplicate-status within the object
    /// identified by `obj_id` (see [`KeyState`]). `next_idx` is
    /// where the key would land in the object's pairs Vec.
    fn intern_key(&mut self, s: &str, obj_id: u64, next_idx: u32) -> KeyHit {
        use std::collections::hash_map::Entry;
        use std::hash::Hasher as _;
        let mut hasher = crate::intern::FxHasher::default();
        hasher.write(s.as_bytes());
        let h = hasher.finish();
        match self.map.entry(h) {
            Entry::Vacant(e) => {
                let val = new_frozen_key(s);
                if self.len < KEY_CACHE_CAP {
                    self.len += 1;
                    e.insert(KeyEntry::One(KeyState {
                        val: val.clone(),
                        last_obj: obj_id,
                        last_idx: next_idx,
                    }));
                    KeyHit::Push(val)
                } else {
                    KeyHit::Uncached(val)
                }
            }
            Entry::Occupied(mut e) => match e.get_mut() {
                KeyEntry::One(st) => {
                    if key_matches(&st.val, s) {
                        hit_state(st, obj_id, next_idx)
                    } else if self.len < KEY_CACHE_CAP {
                        // true fxhash collision: two different key
                        // texts, one hash — keep both.
                        self.len += 1;
                        let val = new_frozen_key(s);
                        let old = std::mem::replace(
                            st,
                            KeyState { val: val.clone(), last_obj: obj_id, last_idx: next_idx },
                        );
                        *e.get_mut() = KeyEntry::Many(vec![
                            old,
                            KeyState { val: val.clone(), last_obj: obj_id, last_idx: next_idx },
                        ]);
                        KeyHit::Push(val)
                    } else {
                        KeyHit::Uncached(new_frozen_key(s))
                    }
                }
                KeyEntry::Many(vs) => {
                    if let Some(st) = vs.iter_mut().find(|st| key_matches(&st.val, s)) {
                        hit_state(st, obj_id, next_idx)
                    } else if self.len < KEY_CACHE_CAP {
                        self.len += 1;
                        let val = new_frozen_key(s);
                        vs.push(KeyState {
                            val: val.clone(),
                            last_obj: obj_id,
                            last_idx: next_idx,
                        });
                        KeyHit::Push(val)
                    } else {
                        KeyHit::Uncached(new_frozen_key(s))
                    }
                }
            },
        }
    }
}

/// Classify a cache hit per the [`KeyState`] clock rules. The
/// ambiguous case leaves the state UNTOUCHED on purpose: a
/// provisional `last_idx` would be wrong when the scan resolves
/// to an existing pair, and a later occurrence would then
/// overwrite the wrong slot.
fn hit_state(st: &mut KeyState, obj_id: u64, next_idx: u32) -> KeyHit {
    use std::cmp::Ordering;
    match st.last_obj.cmp(&obj_id) {
        Ordering::Equal => KeyHit::Dup(st.last_idx),
        Ordering::Less => {
            st.last_obj = obj_id;
            st.last_idx = next_idx;
            KeyHit::Push(st.val.clone())
        }
        Ordering::Greater => KeyHit::Ambiguous(st.val.clone()),
    }
}

/// Build the frozen, UTF-8-tagged key String. Frozen-ness matches
/// CRuby (`JSON.parse('{"a":1}').keys[0].frozen? == true`), and is
/// what makes sharing one Rc across duplicate keys sound — nothing
/// can mutate the text through one key and corrupt the others.
fn new_frozen_key(s: &str) -> Value {
    let rstr = crate::value::RStr::new(s.to_string());
    rstr.frozen.set(true);
    Value::Str(std::rc::Rc::new(rstr))
}

fn key_matches(v: &Value, s: &str) -> bool {
    match v {
        Value::Str(rs) => *rs.content.borrow() == s.as_bytes(),
        _ => false,
    }
}

fn key_ptr_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => std::rc::Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn key_content_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => *x.content.borrow() == *y.content.borrow(),
        _ => false,
    }
}

/// Object-key seed: deserializes a key via a TRANSIENT `&str`
/// (serde_json borrows escape-free keys straight from the input
/// slice; escaped ones come from its scratch buffer) — so repeat
/// keys allocate NOTHING, and first-sight keys allocate exactly
/// one String + Rc. The old `next_key::<String>()` shape paid a
/// String allocation per key occurrence.
struct KeySeed<'a, 'c> {
    ctx: &'a mut ParseCtx<'c>,
    obj_id: u64,
    next_idx: u32,
}

impl<'de> serde::de::DeserializeSeed<'de> for KeySeed<'_, '_> {
    type Value = KeyHit;

    fn deserialize<D>(self, deserializer: D) -> Result<KeyHit, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(self)
    }
}

impl<'de> serde::de::Visitor<'de> for KeySeed<'_, '_> {
    type Value = KeyHit;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON object key")
    }

    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<KeyHit, E> {
        Ok(self.ctx.keys.intern_key(s, self.obj_id, self.next_idx))
    }
}

/// Streaming-visitor parse: skips the `serde_json::Value`
/// intermediate by allocating Ruby `Value`s directly during
/// the serde state walk. ~30 % faster on a 3.4 KB payload than
/// the two-pass form because the Rust-side tree never
/// materialises — Hash / Array allocations land straight on
/// `vm.heap`.
///
/// The `&mut ParseCtx` borrow threads through nested seeds via
/// `VmSeed`: each `next_element_seed` / `next_value_seed`
/// re-borrows from `self.ctx`, so the outer visitor's lifetime
/// stays valid across the recursion.
struct VmVisitor<'a, 'c> {
    ctx: &'a mut ParseCtx<'c>,
}

struct VmSeed<'a, 'c> {
    ctx: &'a mut ParseCtx<'c>,
}

impl<'de> serde::de::DeserializeSeed<'de> for VmSeed<'_, '_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // The CRuby nesting rule (see `ParseCtx::depth`): a value
        // parsed inside more than max_nesting containers raises.
        // This seed only runs for container ELEMENTS / object
        // values, so empty containers at the boundary never
        // trigger it — matching CRuby's non-empty-entry check.
        if self.ctx.depth > MAX_PARSE_NESTING {
            return Err(serde::de::Error::custom(format!(
                "nesting of {} is too deep",
                self.ctx.depth
            )));
        }
        deserializer.deserialize_any(VmVisitor { ctx: self.ctx })
    }
}

impl<'de> serde::de::Visitor<'de> for VmVisitor<'_, '_> {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Nil)
    }
    fn visit_bool<E: serde::de::Error>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }
    fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Value, E> {
        Ok(Value::Int(n))
    }
    #[inline]
    fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Value, E> {
        // serde parses positive integer literals up to u64::MAX
        // exactly — the (i64::MAX, u64::MAX] span becomes an exact
        // Bignum (CRuby parses it as Integer), no retry pass
        // needed. 19–20-digit snowflake IDs land in that arm. The
        // Bignum arm is outlined `#[cold]`: inlining its alloc
        // path here fattened visit_u64 past serde's inline
        // threshold and cost a measured ~14% on a 200-int array
        // fixture (the ubiquitous small-int lane must stay tiny).
        if n <= i64::MAX as u64 {
            Ok(Value::Int(n as i64))
        } else {
            visit_u64_bignum(self.ctx, n)
        }
    }
    #[inline]
    fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Value, E> {
        // Values past the integer-precision thresholds may be
        // silently-rounded bigint literals, and negative zero may
        // be the INTEGER literal "-0" (CRuby: Integer 0) — the
        // f64 alone can't say. `ParseCtx::exact_number` (outlined,
        // cold on the primary pass) resolves via the raw-text
        // queue (retry pass) or requests the retry (primary pass).
        // Ordinary floats — the fast path — pay three predictable
        // compares.
        if f64_needs_exact(n) {
            return self.ctx.exact_number(n).map_err(serde::de::Error::custom);
        }
        Ok(Value::Float(n))
    }
    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Value, E> {
        Ok(Value::new_str(s.to_string()))
    }
    fn visit_string<E: serde::de::Error>(self, s: String) -> Result<Value, E> {
        Ok(Value::new_str(s))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        self.ctx.depth += 1;
        // Pre-size the Vec: serde_json never provides a size hint
        // (JSON arrays are length-unknown until `]`), so default to
        // 8 slots — skips the 0→4→8 growth re-allocs every small
        // array pays (measured ~2-4 µs/iter on 200-object payloads).
        let mut elems: Vec<Value> =
            Vec::with_capacity(seq.size_hint().unwrap_or(8));
        while let Some(v) = seq.next_element_seed(VmSeed { ctx: &mut *self.ctx })? {
            elems.push(v);
        }
        self.ctx.depth -= 1;
        let id = self.ctx.vm.heap.alloc(HeapObj::Array(elems.into()));
        Ok(Value::Array(id))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        self.ctx.depth += 1;
        let obj_id = self.ctx.keys.next_obj_gen();
        // Record-shape fast path: build the SmallVec pairs buffer
        // directly, so a ≤HASH_INLINE_PAIRS object (the JSON-record
        // common case) allocates NO pairs buffer at all — the pairs
        // land inline in the HashObj (and thus inline in the heap
        // slot), and the sweep side frees nothing. Larger objects
        // spill to one heap buffer on the 4th push, like the old
        // pre-sized Vec.
        let mut pairs: crate::heap::PairsBuf = crate::heap::PairsBuf::new();
        loop {
            let seed = KeySeed {
                obj_id,
                next_idx: pairs.len() as u32,
                ctx: &mut *self.ctx,
            };
            let Some(hit) = map.next_key_seed(seed)? else { break };
            let v = map.next_value_seed(VmSeed { ctx: &mut *self.ctx })?;
            // Duplicate keys within ONE object: CRuby is last-wins
            // with the key keeping its original position
            // ('{"a":1,"b":2,"a":3}' → {"a" => 3, "b" => 2}).
            // The KeyState generation clock resolves the common
            // cases O(1); the two scan fallbacks are rare shapes.
            match hit {
                KeyHit::Push(k) => pairs.push((k, v)),
                KeyHit::Dup(idx) => pairs[idx as usize].1 = v,
                KeyHit::Ambiguous(k) => {
                    if let Some(slot) = pairs.iter_mut().find(|(pk, _)| key_ptr_eq(pk, &k)) {
                        slot.1 = v;
                    } else {
                        pairs.push((k, v));
                    }
                }
                KeyHit::Uncached(k) => {
                    if let Some(slot) = pairs.iter_mut().find(|(pk, _)| key_content_eq(pk, &k)) {
                        slot.1 = v;
                    } else {
                        pairs.push((k, v));
                    }
                }
            }
        }
        self.ctx.depth -= 1;
        let id = self.ctx.vm.heap.alloc(HeapObj::Hash(HashObj::with_pairs(pairs)));
        Ok(Value::Hash(id))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    // ---- exponent fence -------------------------------------------------

    #[test]
    fn exp_fence_detection() {
        // Below the fence: everything real payloads carry.
        assert!(!has_exp_out_of_range(b"[1e9]"));
        assert!(!has_exp_out_of_range(b"[1.5e+15,2e-300]"));
        assert!(!has_exp_out_of_range(b"[1e999999999]")); // 9 digits
        assert!(!has_exp_out_of_range(br#"{"score":1.5e15}"#));
        // At/past the fence (>= 10 written exponent digits).
        assert!(has_exp_out_of_range(b"[1e1234567890]"));
        assert!(has_exp_out_of_range(b"[0.0e2147483649]"));
        assert!(has_exp_out_of_range(b"[1e+9999999999]"));
        assert!(has_exp_out_of_range(b"[1E-0000000009]"));
        assert!(has_exp_out_of_range(b"1e1234567890")); // bare top-level
        assert!(has_exp_out_of_range(b"[0.0e999999999999999999]"));
        // Saturation-family literals (>= 19-20 exponent digits).
        assert!(has_exp_out_of_range(b"[1e00000000000000000009]"));
        assert!(has_exp_out_of_range(b"[-0.0e-00000000000000000009]"));
        assert!(has_exp_out_of_range(b"[1.5e-9999999999999999999]"));
        // Digit runs inside STRINGS never trip the fence.
        assert!(!has_exp_out_of_range(br#"["e12345678901"]"#));
        assert!(!has_exp_out_of_range(br#"{"sid":"1234567890123456789","n":1}"#));
        assert!(!has_exp_out_of_range(br#"["1e99999999999999999999 in a string"]"#));
        assert!(!has_exp_out_of_range(br#"["esc\"e12345678901234567890\" more"]"#));
        // Long digit runs WITHOUT an exponent prefix pass (bigints
        // are handled by the exact retry, not a decline) — includes
        // the >40-digit left-expansion-cap path.
        assert!(!has_exp_out_of_range(b"[12345678901234567890123456789]"));
        let sixty = format!("[{}]", "9".repeat(60));
        assert!(!has_exp_out_of_range(sixty.as_bytes()));
        // e-prefixed run straddling the cap still detected.
        let deep = format!("[1e{}]", "9".repeat(60));
        assert!(has_exp_out_of_range(deep.as_bytes()));
    }

    // ---- exact-literal scan ---------------------------------------------

    fn scan(doc: &[u8]) -> Vec<(u64, Exact)> {
        scan_exact_literals(doc)
    }

    #[test]
    fn exact_literal_scan_shapes() {
        // Nothing to queue: i64/u64-range ints + ordinary floats.
        assert!(scan(b"[1,-2,3.5,9223372036854775807,-9223372036854775808]").is_empty());
        assert!(scan(b"[9223372036854775808,18446744073709551615]").is_empty()); // u64 lane
        assert!(scan(br#"{"sid":"1234567890123456789","n":1}"#).is_empty()); // in-string
        assert!(scan(br#"["a\"12345678901234567890123",1]"#).is_empty()); // escaped quote
        // Bigints beyond the native lanes.
        let q = scan(b"[18446744073709551616]");
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].0, 18446744073709551616.0f64.to_bits());
        assert!(matches!(&q[0].1, Exact::Big(t) if &**t == b"18446744073709551616"));
        let q = scan(b"[-9223372036854775809]");
        assert_eq!(q.len(), 1);
        assert!(matches!(&q[0].1, Exact::Big(_)));
        // Negative zero: int spelling -> Int0, float spellings -> Float.
        let q = scan(b"[-0]");
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].0, NEG_ZERO_BITS);
        assert!(matches!(q[0].1, Exact::Int0));
        let q = scan(b"[-0.0,-0e5,-1e-400]");
        assert_eq!(q.len(), 3);
        for (bits, e) in &q {
            assert_eq!(*bits, NEG_ZERO_BITS);
            assert!(matches!(e, Exact::Float));
        }
        // Huge float literals (suspicion range) queue as Float.
        let q = scan(b"[1e20,-9.3e18]");
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].0, 1e20f64.to_bits());
        assert_eq!(q[1].0, (-9.3e18f64).to_bits());
        // Document order is preserved across mixed shapes.
        let q = scan(br#"{"a":-0,"b":[1e20,123456789012345678901234567890],"c":2}"#);
        assert_eq!(q.len(), 3);
        assert!(matches!(q[0].1, Exact::Int0));
        assert!(matches!(q[1].1, Exact::Float));
        assert!(matches!(q[2].1, Exact::Big(_)));
    }

    #[test]
    fn int_native_boundaries() {
        assert!(int_fits_native(false, b"18446744073709551615")); // u64::MAX
        assert!(!int_fits_native(false, b"18446744073709551616"));
        assert!(int_fits_native(true, b"9223372036854775808")); // i64::MIN
        assert!(!int_fits_native(true, b"9223372036854775809"));
        assert!(int_fits_native(false, b"1"));
        assert!(!int_fits_native(false, b"999999999999999999999"));
    }

    // ---- serde <-> from_str f64 equivalence (pairing soundness) ---------

    enum Num {
        I(i64),
        U(u64),
        F(f64),
    }

    struct NumVisitor;
    impl serde::de::Visitor<'_> for NumVisitor {
        type Value = Num;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON number")
        }
        fn visit_i64<E>(self, n: i64) -> Result<Num, E> {
            Ok(Num::I(n))
        }
        fn visit_u64<E>(self, n: u64) -> Result<Num, E> {
            Ok(Num::U(n))
        }
        fn visit_f64<E>(self, n: f64) -> Result<Num, E> {
            Ok(Num::F(n))
        }
    }

    fn serde_num(text: &str) -> Num {
        let mut de = serde_json::Deserializer::from_slice(text.as_bytes());
        let v = serde::de::Deserializer::deserialize_any(&mut de, NumVisitor)
            .unwrap_or_else(|e| panic!("serde parse failed for {text:?}: {e}"));
        de.end().unwrap();
        v
    }

    /// xorshift64* — deterministic, no rand dep.
    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state >> 12;
        *state ^= *state << 25;
        *state ^= *state >> 27;
        state.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// >=1M-sample equivalence: format a random finite f64 with the
    /// canonical fpconv writer (`json_float`), reparse through BOTH
    /// serde_json's visitor path and Rust's `str::parse::<f64>` —
    /// all three bit-identical. This is the float half of the
    /// exact-retry pairing contract: the retry scanner predicts
    /// serde's f64 delivery via `from_str`, so any disagreement
    /// would break bigint/negative-zero documents (they'd decline
    /// to the canon — safe but slow) and this test would catch the
    /// drift.
    #[test]
    fn serde_f64_matches_from_str_on_writer_output() {
        let mut state: u64 = 0xDEADBEEFCAFED00D;
        let mut buf: Vec<u8> = Vec::with_capacity(32);
        let mut tested = 0u64;
        while tested < 1_200_000 {
            let bits = xorshift(&mut state);
            let f = f64::from_bits(bits);
            if f.is_nan() || f.is_infinite() {
                continue;
            }
            buf.clear();
            crate::json_float::write_json_float(f, &mut buf);
            let text = std::str::from_utf8(&buf).expect("writer output is ASCII");
            let direct: f64 = text.parse().expect("writer output parses");
            assert_eq!(direct.to_bits(), f.to_bits(), "from_str drift on {text:?}");
            match serde_num(text) {
                Num::F(g) => assert_eq!(
                    g.to_bits(),
                    f.to_bits(),
                    "serde drift on {text:?} (bits {bits:016x})"
                ),
                _ => panic!("writer output {text:?} did not parse as float"),
            }
            tested += 1;
        }
    }

    /// Bigint-literal half of the pairing contract: for integer
    /// literals beyond the native i64/u64 lanes, serde delivers a
    /// silently-rounded f64 — the retry scanner must predict its
    /// exact bit pattern with `from_str`. Sweeps random 20-40-digit
    /// literals plus the boundary straddles.
    #[test]
    fn serde_f64_matches_from_str_on_bigint_literals() {
        let check = |text: &str| {
            let expected: f64 = text.parse().unwrap();
            match serde_num(text) {
                Num::F(g) => assert_eq!(
                    g.to_bits(),
                    expected.to_bits(),
                    "serde/from_str disagree on {text:?}"
                ),
                Num::U(u) => assert!(
                    u <= u64::MAX,
                    "in-range literal {text:?} stays exact"
                ),
                Num::I(_) => {}
            }
        };
        // Boundary straddles.
        for k in 0..=64u128 {
            check(&format!("{}", (1u128 << 64) + k));
            check(&format!("{}", (1u128 << 64).wrapping_sub(k)));
            check(&format!("-{}", (1u128 << 63) + k));
        }
        // Random 20-40-digit literals, both signs.
        let mut state: u64 = 0x0123456789ABCDEF;
        for i in 0..400_000u64 {
            let len = 20 + (xorshift(&mut state) % 21) as usize;
            let mut s = String::with_capacity(len + 1);
            if i % 2 == 1 {
                s.push('-');
            }
            s.push((b'1' + (xorshift(&mut state) % 9) as u8) as char);
            for _ in 1..len {
                s.push((b'0' + (xorshift(&mut state) % 10) as u8) as char);
            }
            check(&s);
        }
    }
}
