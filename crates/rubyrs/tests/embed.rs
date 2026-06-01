//! Public API smoke tests. Locks down the embedding surface
//! (Runtime, register_fn, set_stdout, eval, format_trap) so it can't
//! regress accidentally.
//!
//! Tests are organised into topical submodules under `tests/embed/`
//! to keep this entry file from growing without bound. Shared
//! helpers (`SharedBuf`, `rt_with_buf`) stay here so every sub-mod
//! can reach them via `super::*`. Each sub-mod registers itself with
//! a `mod` declaration below.

// Topical sub-modules — each owns its own #[test] fns, all linked
// into this same test binary so `cargo test --test embed` still
// runs the full suite. Filter by name: `cargo test --test embed
// adr_0017`.
//
// `#[path = "..."]` is needed because Cargo treats this file as a
// crate root, so `mod x;` would normally look for `tests/x.rs`
// rather than `tests/embed/x.rs`. Pointing the path lets the
// sub-files sit under `tests/embed/` (organisational), keeping the
// `tests/` listing flat for the rest of the integration tests.
#[path = "embed/adr_0017.rs"]
mod adr_0017;
#[path = "embed/dispatch_quirks.rs"]
mod dispatch_quirks;
#[path = "embed/equality.rs"]
mod equality;
#[path = "embed/error_handling.rs"]
mod error_handling;
#[path = "embed/filesystem_sandbox.rs"]
mod filesystem_sandbox;
#[path = "embed/gc.rs"]
mod gc;
#[path = "embed/load_paths.rs"]
mod load_paths;
#[path = "embed/misc.rs"]
mod misc;
#[path = "embed/numeric.rs"]
mod numeric;
#[path = "embed/reset.rs"]
mod reset;
#[path = "embed/resource_caps.rs"]
mod resource_caps;
#[path = "embed/rubund_validation.rs"]
mod rubund_validation;
#[path = "embed/tier1_capability.rs"]
mod tier1_capability;
#[path = "embed/m27_rubyrs_const.rs"]
mod m27_rubyrs_const;

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use rubyrs::{HostCtx, Runtime, RubyError, Trap, Value};

#[derive(Clone)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl SharedBuf {
    fn new() -> Self { SharedBuf(Rc::new(RefCell::new(Vec::new()))) }
    fn snapshot(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).expect("non-utf8 stdout")
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn rt_with_buf() -> (Runtime, SharedBuf) {
    let mut rt = Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    (rt, buf)
}

#[test]
fn eval_runs_a_simple_script() {
    let (mut rt, buf) = rt_with_buf();
    rt.eval(r#"puts "hi""#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "hi\n");
}

#[test]
fn eval_returns_final_value() {
    let mut rt = Runtime::new();
    let v = rt.eval("1 + 2", "t.rb").unwrap();
    assert!(matches!(v, Value::Int(3)));
}

#[test]
fn host_fn_is_callable_from_ruby() {
    let (mut rt, buf) = rt_with_buf();
    rt.register_fn("triple", |args| match args {
        [Value::Int(n)] => Ok(Value::Int(n * 3)),
        _ => Ok(Value::Nil),
    });
    rt.eval(r#"puts triple(7)"#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "21\n");
}

#[test]
fn host_fn_can_propagate_trap() {
    let mut rt = Runtime::new();
    rt.register_fn("boom", |_| {
        // ArgumentError via the public surface
        Err(rubyrs::Trap {
            err: rubyrs::RubyError::ArgumentError { msg: "no good".into() },
            backtrace: vec![],
        })
    });
    let res = rt.eval(r#"boom"#, "t.rb");
    assert!(res.is_err());
    let formatted = rt.format_trap(&res.unwrap_err());
    assert!(formatted.contains("no good"), "got: {}", formatted);
    assert!(formatted.contains("ArgumentError"), "got: {}", formatted);
}

#[test]
fn register_fn_v2_reads_array_arg_via_host_ctx() {
    // v2 signature gives the closure a `HostCtx` so it can unpack
    // `Value::Array` directly — the heap-y shape that v1's
    // `&[Value]`-only signature couldn't reach without going back
    // through the (cloning) `Runtime::resolve_array`. This is the
    // gap PR #35's Gemfile demo hit and worked around with a
    // Ruby-side prelude that flattened `*args` to a `|`-joined
    // String.
    let (mut rt, buf) = rt_with_buf();
    rt.register_fn_v2("sum_array", |ctx: &HostCtx, args: &[Value]| {
        let arr = match args {
            [v] => ctx.resolve_array(v).ok_or_else(|| Trap {
                err: RubyError::ArgumentError { msg: "expected Array".into() },
                backtrace: vec![],
            })?,
            _ => return Err(Trap {
                err: RubyError::ArgumentError { msg: "wrong arity".into() },
                backtrace: vec![],
            }),
        };
        let mut total: i64 = 0;
        for v in arr {
            if let Value::Int(n) = v { total += n; } else {
                return Err(Trap {
                    err: RubyError::TypeError { msg: "expected Integer element".into() },
                    backtrace: vec![],
                });
            }
        }
        Ok(Value::Int(total))
    });
    rt.eval(r#"puts sum_array([1, 2, 3, 4, 5])"#, "t.rb").unwrap();
    assert_eq!(buf.snapshot(), "15\n");
}

#[test]
fn register_fn_v2_reads_hash_arg_via_host_ctx() {
    // Two checks against the same Hash:
    //   1. key lookup returns the right Value (exercises the
    //      resolve_hash → (k, v) pair shape end-to-end).
    //   2. iteration order matches insertion order. CRuby's
    //      Hash guarantees this since 1.9; rubyrs's
    //      `Vec<(Value, Value)>` representation preserves it
    //      mechanically. The `hash_keys` host fn below joins
    //      keys with `|` so a regression that switched to a
    //      `HashMap` or any unordered shape would fail with a
    //      different concrete output string.
    let mut rt = Runtime::new();
    rt.register_fn_v2("hash_lookup", |ctx: &HostCtx, args: &[Value]| {
        let (h, want) = match args {
            [h, Value::Str(s)] => (
                ctx.resolve_hash(h).ok_or_else(|| Trap {
                    err: RubyError::ArgumentError { msg: "expected Hash".into() },
                    backtrace: vec![],
                })?,
                s.to_string_lossy(),
            ),
            _ => return Err(Trap {
                err: RubyError::ArgumentError { msg: "wrong arity / types".into() },
                backtrace: vec![],
            }),
        };
        for (k, v) in h {
            if let Value::Str(ks) = k
                && ks.to_string_lossy() == want
            {
                return Ok(v.clone());
            }
        }
        Ok(Value::Nil)
    });
    // Captures the iteration order Rust-side so the assertion
    // below can read an actual String (sidesteps `Value::Str`'s
    // non-public constructor).
    let captured_keys: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![]));
    let captured_for_fn = captured_keys.clone();
    rt.register_fn_v2("hash_keys", move |ctx: &HostCtx, args: &[Value]| {
        let h = match args {
            [h] => ctx.resolve_hash(h).ok_or_else(|| Trap {
                err: RubyError::ArgumentError { msg: "expected Hash".into() },
                backtrace: vec![],
            })?,
            _ => return Err(Trap {
                err: RubyError::ArgumentError { msg: "wrong arity".into() },
                backtrace: vec![],
            }),
        };
        let mut out = captured_for_fn.borrow_mut();
        out.clear();
        for (k, _) in h {
            if let Value::Str(s) = k { out.push(s.to_string_lossy()); }
        }
        Ok(Value::Nil)
    });

    // Key lookup.
    let v = rt.eval(
        r#"hash_lookup({ "a" => 1, "b" => 2, "c" => 3 }, "b")"#,
        "t.rb",
    ).unwrap();
    assert!(matches!(v, Value::Int(2)), "expected Int(2), got {:?}", v);

    // Iteration order. Insertion order is `a, b, c`; if the
    // underlying representation ever became unordered (e.g.
    // HashMap) this assertion would fail with a different
    // permutation rather than passing silently.
    rt.eval(
        r#"hash_keys({ "a" => 1, "b" => 2, "c" => 3 })"#,
        "t.rb",
    ).unwrap();
    assert_eq!(&*captured_keys.borrow(), &["a", "b", "c"],
        "hash iteration order should match insertion");
}

#[test]
fn register_fn_v2_resolves_sym_arg_via_host_ctx() {
    // `HostCtx::resolve_sym` lets the host borrow the interned name
    // of a `Value::Sym` arg without going through the prelude. This
    // closes the gap PR #40 noted: a Bundler-style kwarg Hash with
    // Symbol keys (`require:`, `platforms:`) can be consumed
    // host-side without a Ruby `k.to_s` rebuild.
    let captured: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(vec![]));
    let captured_for_fn = captured.clone();
    let mut rt = Runtime::new();
    rt.register_fn_v2("sym_names", move |ctx: &HostCtx, args: &[Value]| {
        let mut out = captured_for_fn.borrow_mut();
        out.clear();
        for v in args {
            let name = ctx.resolve_sym(v).ok_or_else(|| Trap {
                err: RubyError::ArgumentError {
                    msg: "expected all args to be Symbols".into(),
                },
                backtrace: vec![],
            })?;
            out.push(name.to_string());
        }
        Ok(Value::Nil)
    });

    rt.eval(r#"sym_names(:require, :platforms, :mri)"#, "t.rb").unwrap();
    assert_eq!(&*captured.borrow(), &["require", "platforms", "mri"]);

    // Negative case: a non-Symbol arg surfaces the explicit
    // ArgumentError, not a silent skip.
    let err = rt.eval(r#"sym_names(:ok, "not a sym")"#, "t.rb").unwrap_err();
    assert!(err.err.is("ArgumentError"),
        "expected ArgumentError on non-Sym arg, got {:?}", err.err);
}

#[test]
fn register_fn_v2_replaces_prior_v1_registration() {
    // Re-registering under the same name swaps the slot —
    // pinning this so a future refactor that uses separate v1/v2
    // maps would have to keep this guarantee explicit.
    let mut rt = Runtime::new();
    rt.register_fn("answer", |_| Ok(Value::Int(42)));
    let v1 = rt.eval(r#"answer"#, "t.rb").unwrap();
    assert!(matches!(v1, Value::Int(42)));

    rt.register_fn_v2("answer", |_ctx, _args| Ok(Value::Int(99)));
    let v2 = rt.eval(r#"answer"#, "t.rb").unwrap();
    assert!(matches!(v2, Value::Int(99)));
}

#[test]
fn register_fn_replaces_prior_v2_registration() {
    // Symmetric direction. Both register_fn / register_fn_v2 doc
    // strings promise replacement either way; without this test a
    // future map-split refactor (separate v1_fns / v2_fns maps)
    // could keep a stale v2 closure live after the embedder
    // re-registered with v1. Existing v1→v2 test above wouldn't
    // catch that direction.
    let mut rt = Runtime::new();
    rt.register_fn_v2("answer", |_ctx, _args| Ok(Value::Int(99)));
    let first = rt.eval(r#"answer"#, "t.rb").unwrap();
    assert!(matches!(first, Value::Int(99)));

    rt.register_fn("answer", |_| Ok(Value::Int(42)));
    let second = rt.eval(r#"answer"#, "t.rb").unwrap();
    assert!(matches!(second, Value::Int(42)),
        "v1 registration should have replaced the prior v2 slot, got {:?}",
        second);
}

#[test]
fn definitions_persist_across_eval() {
    let (mut rt, buf) = rt_with_buf();
    rt.eval(
        r#"
        class Greeter
          def initialize(name); @name = name; end
          def hello; "hi, #{@name}"; end
        end
        "#,
        "define.rb",
    ).unwrap();
    rt.eval(r#"puts Greeter.new("rubyrs").hello"#, "use.rb").unwrap();
    assert_eq!(buf.snapshot(), "hi, rubyrs\n");
}

// ---------- P1-D: resource caps ----------

// ---------- resolve_* helpers ----------

// ---------- Metaprogramming PoCs ----------

// ADR 0017 host-capability defaults — moved to
// `tests/embed/adr_0017.rs` (the `mod adr_0017;` at the top of
// this file pulls them in).

// Tier 1 capability-injection tests (Random / SecureRandom /
// Time) moved to `tests/embed/tier1_capability.rs`.

