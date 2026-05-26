//! `primitive_call` — the typed fast-path dispatch table for
//! built-in receiver methods (Int, Float, String, Symbol, Bool,
//! Nil). Mirrors the per-class C function tables CRuby installs
//! in `numeric.c`, `string.c`, etc., but as a single Rust match
//! so the receiver-type checks short-circuit before any HashMap
//! work.
//!
//! Called from `do_call` / `do_call_block` before any Object/
//! class-table lookup; on `Ok(None)` the dispatch chain falls
//! through to the user-method path.

use std::rc::Rc;

use crate::error::RubyError;
use crate::value::Value;

use super::{numeric, string};

pub(crate) fn primitive_call(recv: &Value, name: &str, args: &[Value], max_value_bytes: Option<usize>) -> Result<Option<Value>, RubyError> {
    // Helper: enforce the per-value byte cap (P2-14c) at every
    // string-growing arm. Returns Err if the projected size would
    // exceed the cap; callers wrap it in `Trap` via `Vm::trap`.
    // Underscore-prefixed: the closure isn't currently called
    // because every path inside primitive_call performs the byte-
    // cap check inline; keeping the helper definition documents
    // the discipline (P2-14c) for future arms and satisfies
    // `-D unused-variables`.
    let _check = |new_len: usize| -> Result<(), RubyError> {
        if let Some(max) = max_value_bytes
            && new_len > max {
                return Err(RubyError::ResourceExhausted {
                    msg: format!("value size {new_len} bytes > cap {max}"),
                });
            }
        Ok(())
    };
    // Per-type sub-dispatchers (mirror CRuby's split). Each
    // returns Some on a hit, None to fall through to the local
    // match for Bool / Nil / Sym / Class / cross-type arms.
    if let Some(v) = numeric::numeric_call(recv, name, args, max_value_bytes)? {
        return Ok(Some(v));
    }
    if let Some(v) = string::string_call(recv, name, args, max_value_bytes)? {
        return Ok(Some(v));
    }
    Ok(match (recv, name, args) {

        (Value::Sym(a), "==", [Value::Sym(b)]) => Some(Value::Bool(a == b)),
        (Value::Sym(a), "!=", [Value::Sym(b)]) => Some(Value::Bool(a != b)),
        (Value::Nil, "to_s", []) => Some(Value::new_str("")),
        (Value::Nil, "inspect", []) => Some(Value::new_str("nil")),
        (Value::Nil, "to_i", []) => Some(Value::Int(0)),
        (Value::Nil, "to_f", []) => Some(Value::Float(0.0)),
        // Bool#inspect — to_s.
        (Value::Bool(b), "inspect", []) => {
            Some(Value::new_str(if *b { "true" } else { "false" }))
        }
        (Value::Nil, "nil?", []) => Some(Value::Bool(true)),
        // Object#nil? is `false` for every non-nil receiver. We
        // implement it here as a generic fallback so e.g.
        // `"abc".nil?` and `5.nil?` work without per-type arms.
        (_, "nil?", []) => Some(Value::Bool(false)),
        // Unary `!`. CRuby defines `Kernel#!` on every Object —
        // `!foo` returns `true` iff `foo` is `nil` or `false`,
        // `false` otherwise. Prism lowers a unary `!` expression
        // as a call to the `!` method, so this universal arm
        // covers every receiver. `!@` (the alternate spelling
        // used by `attr_*` / `define_method`) is the same op.
        (_, "!", []) | (_, "!@", []) => Some(Value::Bool(!recv.is_truthy())),
        (Value::Bool(b), "to_s", []) => Some(Value::new_str(if *b { "true" } else { "false" })),
        // CRuby's TrueClass / FalseClass don't define `<=>`;
        // `Object#<=>` falls back to "0 if identical instance
        // else nil". Booleans are singletons (every `true` is
        // the same instance) so `true <=> true == 0` and
        // `true <=> false == nil`. Same shape for Nil.
        (Value::Bool(a), "<=>", [Value::Bool(b)]) => {
            Some(if a == b { Value::Int(0) } else { Value::Nil })
        }
        (Value::Nil, "<=>", [Value::Nil]) => Some(Value::Int(0)),
        // Per-built-in-lhs catch-alls: when the rhs type doesn't
        // match any specific arm above, `<=>` is `nil`, not
        // NoMethodError. We have to enumerate per-lhs (rather
        // than a universal `(_, "<=>", _)`) so that user-defined
        // `<=>` on `Value::Object` still wins via the normal
        // class-method-lookup path in `do_call`.
        (Value::Int(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Float(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Str(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Bool(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Nil, "<=>", [_]) => Some(Value::Nil),
        (Value::Class(c), "name", []) | (Value::Class(c), "to_s", []) | (Value::Class(c), "inspect", []) => {
            Some(Value::new_str(c.name.clone()))
        }
        // Class identity is `Rc::ptr_eq` — two `Value::Class` refer
        // to the same class iff they point at the same `Rc<Class>`.
        // Reopened classes share the same Rc by virtue of the
        // class-table lookup in `Op::DefClass`, so
        // `class Foo; end; class Foo; end; Foo == Foo` is `true`.
        (Value::Class(a), "==", [Value::Class(b)]) => Some(Value::Bool(Rc::ptr_eq(a, b))),
        (Value::Class(a), "!=", [Value::Class(b)]) => Some(Value::Bool(!Rc::ptr_eq(a, b))),
        _ => None,
    })
}
