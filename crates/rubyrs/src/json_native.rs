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
            write_value(vm, v, &mut out, 0)?;
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
        // Bignum guard: serde_json (without arbitrary_precision)
        // parses integer literals past u64 range via visit_f64 —
        // SILENT precision loss where CRuby produces an exact
        // Integer/Bignum. Any document containing a ≥19-digit run
        // declines to the pure canon (which folds digits with
        // promoting Integer arithmetic). ≤18-digit integers always
        // fit i64, so the fast path keeps exact semantics. False
        // positives (long digit runs inside strings / float
        // fractions) only cost speed, never correctness. The scan
        // strides 19 bytes and only expands around digit hits —
        // ~n/19 byte touches on non-numeric payloads.
        if has_long_digit_run(&s.content.borrow()) {
            return Err(Trap {
                err: RubyError::RuntimeError {
                    msg: "json_native: bigint-range number".to_string(),
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
        // before this closure runs; the &mut borrow lasts only
        // for the deserialize call's synchronous duration and
        // isn't stashed anywhere.
        let vm = unsafe { &mut *ptr };
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
        vm.maybe_gc();
        // Borrow the input bytes directly (no copy). The `Ref`
        // lives across the visitor's `&mut vm` use — that's fine:
        // the RStr is Rc-owned by the caller's argument slot (not
        // a GC-heap object), no Ruby code runs inside the visitor,
        // and `Heap::alloc` never collects, so nothing can mutate
        // or free the buffer mid-parse.
        let bytes = s.content.borrow();
        let mut de = serde_json::Deserializer::from_slice(&bytes);
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
        KEY_CACHE.with(|kc| {
            let mut kc = kc.borrow_mut();
            let mut ctx = ParseCtx {
                vm,
                depth: 0,
                keys: &mut kc,
            };
            let visitor = VmVisitor { ctx: &mut ctx };
            let result = serde::de::Deserializer::deserialize_any(&mut de, visitor)
                .map_err(map_parse_err)?;
            de.end().map_err(map_parse_err)?;
            Ok(result)
        })
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
                write_value(vm, item, out, depth + 1)?;
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
                write_value(vm, val, out, depth + 1)?;
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

/// True when `bytes` contains a run of ≥19 consecutive ASCII
/// digits (the shortest run that can overflow an i64 with a
/// leading `-`) in NUMBER context — i.e. outside JSON string
/// literals. Two tiers:
///
///   1. Cheap filter: a strided scan (a 19-byte window always
///      contains exactly one position ≡ 18 mod 19, so checking
///      every 19th byte and expanding around digit hits touches
///      ~n/19 bytes) finds any ≥19-digit run REGARDLESS of
///      context. No hit — the overwhelmingly common case — costs
///      ~0.1 µs on a 3.4 KB payload and the parse proceeds.
///   2. Precise pass, only on filter hit: a byte state machine
///      tracks in-string state (escape-aware) and counts digit
///      runs outside strings. Long IDs in string values
///      ("sid":"1234567890123456789" — snowflake/Stripe-shaped
///      payloads) no longer decline the whole document to the
///      200×-slower canon (a measured 160× parse regression
///      before this tier existed).
///
/// False positives from tier 2 (a genuine ≥19-digit run outside a
/// string in a MALFORMED document) still just decline to the
/// canon, whose strict number grammar raises like CRuby.
fn has_long_digit_run(bytes: &[u8]) -> bool {
    let n = bytes.len();
    let mut i = 18usize;
    let mut filter_hit = false;
    while i < n {
        if bytes[i].is_ascii_digit() {
            let mut lo = i;
            while lo > 0 && bytes[lo - 1].is_ascii_digit() {
                lo -= 1;
            }
            let mut hi = i + 1;
            while hi < n && bytes[hi].is_ascii_digit() {
                hi += 1;
            }
            if hi - lo >= 19 {
                filter_hit = true;
                break;
            }
            // Next possible 19-run starts after this run's end
            // (bytes[hi] is a non-digit); its last byte is at
            // hi + 19.
            i = hi + 19;
        } else {
            i += 19;
        }
    }
    if !filter_hit {
        return false;
    }
    // Tier 2: string-aware recount. Split into two tight inner
    // loops (outside-string digit counting / inside-string skip)
    // so the hot in-string path is a 3-way branch instead of a
    // state-flag ladder — measured ~2× faster on string-heavy
    // payloads.
    let mut i = 0usize;
    'outside: while i < n {
        let mut run = 0usize;
        while i < n {
            let b = bytes[i];
            if b.is_ascii_digit() {
                run += 1;
                if run >= 19 {
                    return true;
                }
                i += 1;
            } else if b == b'"' {
                i += 1;
                // inside a string literal: skip to the closing
                // unescaped quote (backslash consumes 2 bytes —
                // covers \\ and \" alike)
                while i < n {
                    let b = bytes[i];
                    if b == b'\\' {
                        i += 2;
                    } else if b == b'"' {
                        i += 1;
                        continue 'outside;
                    } else {
                        i += 1;
                    }
                }
                return false;
            } else {
                run = 0;
                i += 1;
            }
        }
    }
    false
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
/// DO hold heap `ObjId`s are safe for the same reason they always
/// were — `Heap::alloc` never collects (collections only run at
/// interpreter safepoints, and no Ruby code runs inside the
/// visitor).
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
    fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Value, E> {
        // The bigint pre-scan declines any document with a ≥19-digit
        // run to the pure canon, so integer literals reaching serde
        // always fit i64 (≤18 digits). Defensive: if a >i64::MAX
        // value somehow arrives, decline rather than silently
        // truncate or promote to Float (CRuby parses it as Integer).
        if n <= i64::MAX as u64 {
            Ok(Value::Int(n as i64))
        } else {
            Err(serde::de::Error::custom("bigint-range number"))
        }
    }
    fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Value, E> {
        // serde_json parses the INTEGER literal "-0" as f64 -0.0
        // (sign-preserving) — indistinguishable here from the
        // float literals "-0.0" / "-0e5". CRuby's parser returns
        // Integer 0 for "-0" and Float -0.0 for the float
        // spellings, so negative zero declines to the canon,
        // which re-reads the actual token text. Rare literal;
        // costs nothing on the fast path.
        if n == 0.0 && n.is_sign_negative() {
            return Err(serde::de::Error::custom("negative-zero literal"));
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
        // Pre-size like visit_seq — 8 covers typical record objects.
        let mut pairs: Vec<(Value, Value)> =
            Vec::with_capacity(map.size_hint().unwrap_or(8));
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

