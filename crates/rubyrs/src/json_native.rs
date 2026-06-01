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
        let parsed: serde_json::Value = serde_json::from_str(&s).map_err(|e| Trap {
            // ParserError is the right class but we surface as
            // RuntimeError here — the canon's wrapper catches
            // and re-raises as JSON::ParserError so the user
            // sees the documented surface.
            err: RubyError::RuntimeError {
                msg: format!("native parse: {e}"),
            },
            backtrace: vec![],
        })?;
        // Build the Ruby value tree. Uses `current_vm_ptr()` —
        // the cext escape hatch (ADR 0013) — to reach the Vm's
        // heap from a v1 host fn. Safe because the dispatch site
        // installs the ptr before invoking the closure (see
        // `Vm::invoke_host_fn`'s `with_vm_ptr_set` guard).
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
        // for the build_value call's synchronous duration and is
        // not stashed anywhere.
        let vm = unsafe { &mut *ptr };
        Ok(build_value(vm, &parsed))
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

/// Convert a `serde_json::Value` into a Ruby `Value`. Allocates
/// String / Array / Hash on `vm.heap`. Hash keys are emitted as
/// `Value::Str` to match the canon's default
/// (`symbolize_names: false`); the canon's parse wrapper handles
/// the `symbolize_names: true` post-pass by re-walking the tree
/// when needed.
fn build_value(vm: &mut crate::vm::Vm, v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::Nil // unreachable for valid JSON
            }
        }
        serde_json::Value::String(s) => Value::new_str(s.clone()),
        serde_json::Value::Array(items) => {
            let elems: Vec<Value> = items.iter().map(|x| build_value(vm, x)).collect();
            let id = vm.heap.alloc(HeapObj::Array(elems));
            Value::Array(id)
        }
        serde_json::Value::Object(map) => {
            let pairs: Vec<(Value, Value)> = map
                .iter()
                .map(|(k, val)| (Value::new_str(k.clone()), build_value(vm, val)))
                .collect();
            let id = vm.heap.alloc(HeapObj::Hash(HashObj::with_pairs(pairs)));
            Value::Hash(id)
        }
    }
}

