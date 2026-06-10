//! `_liquid_native` — liquidus-backed accelerator for Liquid template
//! rendering under Jekyll.
//!
//! ADR 0019 Rule 6 partition (fifth sibling: _json_native →
//! _rouge_native → _kramdown_native → _yaml_native → here): the
//! pure-Ruby liquid gem stays the spec; this battery is the
//! behaviour-equivalent fast path. After `require "jekyll"` completes,
//! a shim (`liquid_native_shim.rb`) patches
//! `Jekyll::LiquidRenderer::File`: at parse time a template compiles
//! through liquidus (whole-template DECLINE for anything outside the
//! subset), and at render time the shim resolves the template's
//! statically-known variable paths from the Liquid payload ONCE,
//! passes them as a plain Ruby Hash, and the host renders natively —
//! the heap is read directly (json_native's pattern), so there is no
//! serialization layer.
//!
//! Host fns:
//!   - `__rubyrs_liquid_compile(src, baseurl, includes_dir) →
//!     Integer | nil` — compile; nil = declined (the shim caches the
//!     decline and the template stays pure-liquid). Includes resolve
//!     against `includes_dir` at compile time.
//!   - `__rubyrs_liquid_needs(tid) → String` — the template's
//!     variable needs, one per line:
//!     `path \t slice|- \t 0|1(need_size) \t field,field,…`.
//!   - `__rubyrs_liquid_render(tid, values) → String | nil` — render
//!     with `values` = Hash{path → value} (plus `path#size`
//!     companions). Value shapes outside the model (or a liquidus
//!     runtime decline) return nil and the shim falls back to pure
//!     liquid for that render.

#![cfg(feature = "_liquid_native")]

use std::cell::RefCell;

use crate::error::{RubyError, Trap};
use crate::value::Value;
use crate::vm::current_vm_ptr;
use liquidus::{LValue, SiteConfig, Values};

/// The Ruby shim injected after `require "jekyll"` (see the hook in
/// `vm/kernel.rs::require_ruby`).
pub(crate) const SHIM: &str = include_str!("liquid_native_shim.rb");

thread_local! {
    /// Compiled templates, indexed by the id handed back to Ruby.
    /// Layout/include templates are few and live for the build;
    /// per-document content templates are short-circuited Ruby-side
    /// (no-tag fast path) and never reach the registry.
    static TEMPLATES: RefCell<Vec<liquidus::Template>> = const { RefCell::new(Vec::new()) };
}

fn arg_err(msg: &str) -> Trap {
    Trap {
        err: RubyError::ArgumentError {
            msg: msg.to_string(),
        },
        backtrace: vec![],
    }
}

/// Register the `__rubyrs_liquid_*` host fns on `rt`. Idempotent. The
/// shim detects registration via `defined?(...)` and stays inert when
/// absent.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    rt.register_fn("__rubyrs_liquid_compile", |args| {
        let (src, baseurl, includes_dir) = match args {
            [Value::Str(s), Value::Str(b), Value::Str(d)] => (
                s.to_string_lossy(),
                b.to_string_lossy(),
                d.to_string_lossy(),
            ),
            _ => {
                return Err(arg_err(
                    "__rubyrs_liquid_compile(src, baseurl, includes_dir)",
                ));
            }
        };
        let include = |name: &str| {
            // Compile-time include resolution. Reject anything that
            // could escape the includes dir; missing files decline
            // the template (pure liquid will raise the proper error).
            if name.contains("..") || name.starts_with('/') {
                return None;
            }
            std::fs::read_to_string(format!("{includes_dir}/{name}")).ok()
        };
        match liquidus::compile(
            &src,
            SiteConfig {
                baseurl: baseurl.to_string(),
            },
            &include,
        ) {
            Ok(tpl) => {
                let id = TEMPLATES.with(|t| {
                    let mut t = t.borrow_mut();
                    t.push(tpl);
                    t.len() - 1
                });
                Ok(Value::Int(id as i64))
            }
            // Out-of-subset template: per-template decline.
            Err(_) => Ok(Value::Nil),
        }
    });

    rt.register_fn("__rubyrs_liquid_needs", |args| {
        let tid = match args {
            [Value::Int(t)] => *t,
            _ => return Err(arg_err("__rubyrs_liquid_needs(tid)")),
        };
        let out = TEMPLATES.with(|t| {
            let t = t.borrow();
            let tpl = usize::try_from(tid).ok().and_then(|i| t.get(i))?;
            let mut out = String::new();
            for need in tpl.variables() {
                out.push_str(&need.path);
                out.push('\t');
                match need.slice {
                    Some(n) => out.push_str(&n.to_string()),
                    None => out.push('-'),
                }
                out.push('\t');
                out.push(if need.need_size { '1' } else { '0' });
                out.push('\t');
                out.push_str(&need.fields.join(","));
                out.push('\n');
            }
            Some(out)
        });
        match out {
            Some(s) => Ok(Value::new_str(s)),
            None => Err(arg_err("liquid_native: bad template id")),
        }
    });

    rt.register_fn("__rubyrs_liquid_render", |args| {
        let (tid, hash_id) = match args {
            [Value::Int(t), Value::Hash(h)] => (*t, *h),
            _ => return Err(arg_err("__rubyrs_liquid_render(tid, values_hash)")),
        };
        let ptr = current_vm_ptr();
        if ptr.is_null() {
            return Err(arg_err("liquid_native: CURRENT_VM_PTR null"));
        }
        // SAFETY: set by the dispatch site immediately before this
        // closure runs; the borrow lasts only for this synchronous
        // call (json_native's pattern).
        let vm = unsafe { &mut *ptr };
        let mut values = Values::default();
        {
            let pairs = vm.heap.hash(hash_id).clone();
            for (k, v) in &pairs {
                let Value::Str(key) = k else {
                    return Ok(Value::Nil);
                };
                let Some(lv) = to_lvalue(vm, v, 0) else {
                    return Ok(Value::Nil);
                };
                values.0.insert(key.to_string_lossy(), lv);
            }
        }
        let rendered = TEMPLATES.with(|t| {
            let t = t.borrow();
            let tpl = usize::try_from(tid).ok().and_then(|i| t.get(i))?;
            tpl.render(&values).ok()
        });
        Ok(match rendered {
            Some(html) => Value::new_str(html),
            // Runtime decline (value shape) — shim falls back.
            None => Value::Nil,
        })
    });
}

/// Convert a VM value into the liquidus model. `None` declines the
/// render (the shim falls back to pure liquid — never guess).
fn to_lvalue(vm: &mut crate::vm::Vm, v: &Value, depth: usize) -> Option<LValue> {
    if depth > 16 {
        return None;
    }
    Some(match v {
        Value::Nil => LValue::Nil,
        Value::Bool(b) => LValue::Bool(*b),
        Value::Int(n) => LValue::Int(*n),
        Value::Float(f) => LValue::Float(*f),
        Value::Str(s) => {
            let bytes = s.borrow();
            LValue::Str(std::str::from_utf8(&bytes).ok()?.to_string())
        }
        Value::Array(id) => {
            let items = vm.heap.array(*id).clone();
            let mut out = Vec::with_capacity(items.len());
            for item in &items {
                out.push(to_lvalue(vm, item, depth + 1)?);
            }
            LValue::Array(out)
        }
        Value::Hash(id) => {
            let pairs = vm.heap.hash(*id).clone();
            let mut out = Vec::with_capacity(pairs.len());
            for (k, val) in &pairs {
                let Value::Str(key) = k else { return None };
                out.push((key.to_string_lossy(), to_lvalue(vm, val, depth + 1)?));
            }
            LValue::Map(out)
        }
        Value::Object(id) => {
            // The one object shape the model accepts: the preamble
            // Time class (clock in @sec, flavour in @local).
            let inst = vm.heap.instance(*id);
            if inst.class.name != "Time" {
                return None;
            }
            let sec_id = vm.interner.intern("@sec");
            let local_id = vm.interner.intern("@local");
            let inst = vm.heap.instance(*id);
            let Some(Value::Int(sec)) = inst.ivars.get(&sec_id).cloned() else {
                return None;
            };
            let local = matches!(inst.ivars.get(&local_id), Some(Value::Bool(true)));
            LValue::Time { sec, local }
        }
        _ => return None,
    })
}
