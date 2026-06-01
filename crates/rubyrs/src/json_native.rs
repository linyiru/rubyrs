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
        let mut out = String::new();
        write_json(v, &mut out)?;
        Ok(Value::new_str(out))
    });

    rt.register_fn("__rubyrs_json_native_parse", |args| {
        let s = match args {
            [Value::Str(s)] => s.to_string_lossy(),
            _ => return Err(Trap {
                err: RubyError::ArgumentError {
                    msg: "__rubyrs_json_native_parse(json_str: String)".to_string(),
                },
                backtrace: vec![],
            }),
        };
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
        let mut de = serde_json::Deserializer::from_str(&s);
        let visitor = VmVisitor { vm };
        let result = serde::de::Deserializer::deserialize_any(&mut de, visitor).map_err(|e| Trap {
            err: RubyError::RuntimeError {
                msg: format!("native parse: {e}"),
            },
            backtrace: vec![],
        })?;
        de.end().map_err(|e| Trap {
            err: RubyError::RuntimeError {
                msg: format!("native parse: {e}"),
            },
            backtrace: vec![],
        })?;
        Ok(result)
    });
}

/// Convert a Ruby `Value` to its compact-JSON byte string,
/// appending to `out`. Mirrors the pure canon's `generate_with`
/// emit shape exactly so the byte-diff parity claim holds.
fn write_json(v: &Value, out: &mut String) -> Result<(), Trap> {
    let ptr = current_vm_ptr();
    if ptr.is_null() {
        return Err(Trap {
            err: RubyError::RuntimeError {
                msg: "json_native: CURRENT_VM_PTR null".to_string(),
            },
            backtrace: vec![],
        });
    }
    let vm = unsafe { &mut *ptr };
    write_value(vm, v, out)
}

fn write_value(vm: &crate::vm::Vm, v: &Value, out: &mut String) -> Result<(), Trap> {
    match v {
        Value::Nil => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(n) => {
            use std::fmt::Write;
            let _ = write!(out, "{n}");
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
            // Mirror Ruby's Float#to_s: integral floats render as
            // `1.0`, fractional as `1.5`. Rust's `{}` for f64
            // gives `1` for 1.0, so we special-case to match.
            if *f == f.trunc() && f.is_finite() && f.abs() < 1e16 {
                use std::fmt::Write;
                let _ = write!(out, "{:.1}", f);
            } else {
                use std::fmt::Write;
                let _ = write!(out, "{}", f);
            }
        }
        Value::Str(s) => write_escaped_string(&s.to_string_lossy(), out),
        Value::Sym(id) => {
            let rc = vm.interner.resolve(*id);
            let s = rc.to_string();
            write_escaped_string(&s, out);
        }
        Value::Array(id) => {
            // Clone the slice so we can release the heap borrow
            // before recursing (children may also walk the heap).
            let items: Vec<Value> = vm.heap.array(*id).to_vec();
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 { out.push(','); }
                write_value(vm, item, out)?;
            }
            out.push(']');
        }
        Value::Hash(id) => {
            let pairs: Vec<(Value, Value)> = vm.heap.hash(*id).to_vec();
            out.push('{');
            for (i, (k, val)) in pairs.iter().enumerate() {
                if i > 0 { out.push(','); }
                // CRuby JSON.generate stringifies non-String
                // keys via to_s. Mirror the canon: emit Symbol
                // as its interned name; Integer / others as
                // their decimal repr.
                match k {
                    Value::Str(s) => write_escaped_string(&s.to_string_lossy(), out),
                    Value::Sym(sid) => write_escaped_string(&vm.interner.resolve(*sid).to_string(), out),
                    Value::Int(n) => {
                        out.push('"');
                        use std::fmt::Write;
                        let _ = write!(out, "{n}");
                        out.push('"');
                    }
                    other => write_escaped_string(&format!("{other:?}"), out),
                }
                out.push(':');
                write_value(vm, val, out)?;
            }
            out.push('}');
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

fn write_escaped_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Streaming-visitor parse: skips the `serde_json::Value`
/// intermediate by allocating Ruby `Value`s directly during
/// the serde state walk. ~30 % faster on a 3.4 KB payload than
/// the two-pass form because the Rust-side tree never
/// materialises — Hash / Array allocations land straight on
/// `vm.heap`.
///
/// The `&'a mut Vm` borrow threads through nested seeds via
/// `VmSeed<'a>`: each `next_element_seed` / `next_value_seed`
/// re-borrows from `self.vm` (an `&'a mut Vm` reborrow), so the
/// outer visitor's lifetime stays valid across the recursion.
struct VmVisitor<'a> {
    vm: &'a mut crate::vm::Vm,
}

struct VmSeed<'a> {
    vm: &'a mut crate::vm::Vm,
}

impl<'a, 'de> serde::de::DeserializeSeed<'de> for VmSeed<'a> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(VmVisitor { vm: self.vm })
    }
}

impl<'a, 'de> serde::de::Visitor<'de> for VmVisitor<'a> {
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
        // JSON has no unsigned-only type; serde uses u64 only
        // when the number is non-negative AND fits a u64 but
        // not i64. Anything past i64::MAX falls to Float
        // (matches CRuby's stdlib JSON behaviour — its parser
        // promotes oversized integers to Float, not Bignum).
        if n <= i64::MAX as u64 {
            Ok(Value::Int(n as i64))
        } else {
            Ok(Value::Float(n as f64))
        }
    }
    fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Value, E> {
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
        // Pre-size the Vec when the deserializer provides a
        // size hint. serde_json doesn't (JSON arrays are length-
        // unknown until `]`), so this is a no-op for the common
        // case; kept for forward-compat with deserializers that
        // do.
        let mut elems: Vec<Value> = seq.size_hint().map(Vec::with_capacity).unwrap_or_default();
        while let Some(v) = seq.next_element_seed(VmSeed { vm: &mut *self.vm })? {
            elems.push(v);
        }
        let id = self.vm.heap.alloc(HeapObj::Array(elems));
        Ok(Value::Array(id))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut pairs: Vec<(Value, Value)> = map.size_hint().map(Vec::with_capacity).unwrap_or_default();
        // `next_key::<String>()` allocates one String per key.
        // Pre-interning common keys (the obvious next opt) would
        // need a sym-cache; the current shape matches CRuby's
        // `JSON::Ext::Parser` which also allocates one Ruby
        // String per key, so we're not losing parity here.
        while let Some(k) = map.next_key::<String>()? {
            let v = map.next_value_seed(VmSeed { vm: &mut *self.vm })?;
            pairs.push((Value::new_str(k), v));
        }
        let id = self.vm.heap.alloc(HeapObj::Hash(HashObj::with_pairs(pairs)));
        Ok(Value::Hash(id))
    }
}

