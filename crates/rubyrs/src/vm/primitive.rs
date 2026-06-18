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
        // nil / true / false render to US-ASCII strings in CRuby (the
        // content is fixed ASCII), not UTF-8.
        (Value::Nil, "to_s", []) => Some(Value::new_str_us_ascii("")),
        (Value::Nil, "inspect", []) => Some(Value::new_str_us_ascii("nil")),
        (Value::Nil, "to_i", []) => Some(Value::Int(0)),
        (Value::Nil, "to_f", []) => Some(Value::Float(0.0)),
        // Bool#inspect — to_s.
        (Value::Bool(b), "inspect", []) => {
            Some(Value::new_str_us_ascii(if *b { "true" } else { "false" }))
        }
        (Value::Nil, "nil?", []) => Some(Value::Bool(true)),
        // Boolean / NilClass logical METHODS (`true & x`, `nil | x`,
        // `false ^ x`). Unlike the short-circuiting `&&`/`||` operators,
        // these always evaluate the argument and test its TRUTHINESS
        // (any object allowed): `true & 1` → true, `nil | "x"` → true.
        // nil behaves exactly like false (CRuby defines them on both).
        (Value::Bool(a), "&", [other]) => Some(Value::Bool(*a && other.is_truthy())),
        (Value::Bool(a), "|", [other]) => Some(Value::Bool(*a || other.is_truthy())),
        (Value::Bool(a), "^", [other]) => Some(Value::Bool(*a != other.is_truthy())),
        (Value::Nil, "&", [_]) => Some(Value::Bool(false)),
        (Value::Nil, "|", [other]) => Some(Value::Bool(other.is_truthy())),
        (Value::Nil, "^", [other]) => Some(Value::Bool(other.is_truthy())),
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
        (Value::Bool(b), "to_s", []) => Some(Value::new_str_us_ascii(if *b { "true" } else { "false" })),
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
        // class-method-lookup path in `do_call`. Also: skip the
        // Int catch-all when rhs is a BigInt — the
        // `Vm::bigint_primitive` hook in `do_call` handles the
        // Int×BigInt case downstream; matching here would
        // shadow it with a wrong `nil` (the Int and BigInt arms
        // for `<=>` both want to compare numerically).
        #[cfg(feature = "bignum")]
        (Value::Int(_), "<=>", [Value::BigInt(_)]) => None,
        // Skip the Int catch-all when rhs is Rational — Phase C.2
        // wires the reverse cross-multiply in the Rational dispatch
        // block in dispatch.rs. Matching here would shadow it with
        // a wrong `nil`.
        (Value::Int(_), "<=>", [Value::Rational(_)]) => None,
        (Value::Int(_), "<=>", [_]) => Some(Value::Nil),
        // Skip the Float catch-all when rhs is BigInt — bigint_primitive's
        // `<=>` arm (via bigint_cmp_float_lossless) handles the
        // Float×BigInt case downstream. Matching here would shadow it
        // with a wrong `nil` (Float and BigInt are both numeric and
        // should compare losslessly).
        #[cfg(feature = "bignum")]
        (Value::Float(_), "<=>", [Value::BigInt(_)]) => None,
        // Same as the Int arm above — Phase C.2 handles Float ×
        // Rational in dispatch.rs's Rational block (after promoting
        // both sides to f64).
        (Value::Float(_), "<=>", [Value::Rational(_)]) => None,
        (Value::Float(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Str(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Bool(_), "<=>", [_]) => Some(Value::Nil),
        (Value::Nil, "<=>", [_]) => Some(Value::Nil),
        // Anonymous modules / classes carry an empty `name` field
        // (the sentinel that `Module.new` writes when no constant
        // assignment promotes a real name). CRuby's contract:
        //   - `.name` returns `nil` for anonymous receivers.
        //   - `.to_s` / `.inspect` return a placeholder
        //     `"#<Module>"` / `"#<Class>"` rather than the empty
        //     string. We don't render the object id like CRuby
        //     does — that's a non-deterministic side that ADR 0017
        //     keeps out of Tier 1.
        (Value::Class(c), "name", []) => {
            // CRuby's `A.singleton_class.name` is `nil` for any
            // eigenclass-shell, even though its `to_s`/`inspect`
            // renders "#<Class:A>". rubyrs stores the shell's
            // display name in the `name` field for diagnostics
            // (see `singleton_class` arm in dispatch.rs); detect
            // the shell via `singleton_target` and return nil
            // here to match CRuby. (Code-review #253 round 6 #1.)
            // CRuby returns nil for anonymous classes AND for
            // per-object singleton shells (the latter detected
            // via `singleton_target`). Both branches collapse
            // to Some(Value::Nil); named non-shell classes
            // return the name.
            // Singleton-class shells are always nil. Otherwise the
            // effective name (structural `name`, or the
            // `assigned_name` stamped on first const-assignment per
            // CRuby — `C = Class.new` ⇒ `C.name == "C"`); a still-
            // anonymous class has neither and reports nil.
            if c.singleton_target.borrow().is_some() {
                Some(Value::Nil)
            } else {
                match c.effective_name() {
                    Some(n) => Some(Value::new_str(n)),
                    None => Some(Value::Nil),
                }
            }
        }
        (Value::Class(c), "to_s", []) | (Value::Class(c), "inspect", []) => {
            Some(Value::new_str(crate::value::class_display_name(c)))
        }
        // Class identity is `Rc::ptr_eq` — two `Value::Class` refer
        // to the same class iff they point at the same `Rc<Class>`.
        // Reopened classes share the same Rc by virtue of the
        // class-table lookup in `Op::DefClass`, so
        // `class Foo; end; class Foo; end; Foo == Foo` is `true`.
        (Value::Class(a), "==", [Value::Class(b)]) => Some(Value::Bool(Rc::ptr_eq(a, b))),
        // Regexp equality is source + flags, not identity —
        // minitest's register_spec_type matcher table is searched
        // with include?([//, Spec]).
        #[cfg(feature = "regex")]
        (Value::Regex(a), "==", [Value::Regex(b)])
        | (Value::Regex(a), "eql?", [Value::Regex(b)]) => {
            Some(Value::Bool(a.as_str() == b.as_str() && a.options() == b.options()))
        }
        #[cfg(feature = "regex")]
        (Value::Regex(a), "!=", [Value::Regex(b)]) => {
            Some(Value::Bool(!(a.as_str() == b.as_str() && a.options() == b.options())))
        }
        (Value::Class(a), "!=", [Value::Class(b)]) => Some(Value::Bool(!Rc::ptr_eq(a, b))),
        _ => None,
    })
}
