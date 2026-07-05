//! HOST-raised Traps crossing `ensure` walks — the embed-level
//! regression battery for "ICE: EndEnsure with empty stack on
//! exception path" (observed via net/http: a socket host fn
//! surfacing an unusual error inside ensure-laden cleanup code).
//!
//! The contract under test (see `Vm::unwind_with_exception` /
//! `SuspendCoord`):
//!   - a host-fn Trap raised AND rescued entirely inside an ensure
//!     body that a `return`/`break` walk is suspended in must NOT
//!     cancel the walk (CRuby resumes the return);
//!   - a host-fn Trap that ESCAPES the ensure body must cancel the
//!     walk (the raise wins);
//!   - a host-fn Trap during an ensure entered on the EXCEPTION
//!     path leaves the original exception re-raise intact when the
//!     host error is rescued within the body.
//!
//! These shapes need `register_fn` (a Trap crossing the host-fn
//! boundary, not a Ruby `raise`), so they live here rather than in
//! the plain-Ruby diff fixtures.

use rubyrs::{RubyError, Runtime, Trap, Value};

fn rt_with_flaky_host_fn() -> Runtime {
    let mut rt = Runtime::new();
    // Raises an IOError-shaped host exception when passed `true`,
    // mimicking a socket host fn hitting an odd external error.
    rt.register_fn("host_close", |args| {
        if matches!(args.first(), Some(Value::Bool(true))) {
            Err(Trap {
                err: RubyError::HostException {
                    class_name: "IOError".into(),
                    message: "stream closed by peer".into(),
                },
                backtrace: vec![],
            })
        } else {
            Ok(Value::Nil)
        }
    });
    rt
}

/// Assert the eval result is the given Symbol.
fn assert_sym(rt: &mut Runtime, val: &Value, expected: &str) {
    match val {
        Value::Sym(id) => assert_eq!(rt.resolve_sym(*id), expected),
        other => panic!("expected :{expected}, got {other:?}"),
    }
}

#[test]
fn host_trap_rescued_inside_return_ensure_resumes_return() {
    let mut rt = rt_with_flaky_host_fn();
    let v = rt
        .eval(
            r#"
            def transfer
              return :sent
            ensure
              begin
                host_close(true)
              rescue IOError
                # swallowed — cleanup errors must not eat the return
              end
            end
            transfer
            "#,
            "t.rb",
        )
        .expect("contained host trap must not surface");
    assert_sym(&mut rt, &v, "sent");
}

#[test]
fn host_trap_rescued_in_callee_of_return_ensure_resumes_return() {
    // The rescue lives one call deeper (net/protocol style: cleanup
    // helper swallows socket errors) — the unwind never touches the
    // suspended frame at all.
    let mut rt = rt_with_flaky_host_fn();
    let v = rt
        .eval(
            r#"
            def safe_close
              host_close(true)
            rescue IOError
              :closed_dirty
            end
            def transfer
              return :sent
            ensure
              safe_close
            end
            transfer
            "#,
            "t.rb",
        )
        .expect("callee-contained host trap must not surface");
    assert_sym(&mut rt, &v, "sent");
}

#[test]
fn host_trap_escaping_return_ensure_cancels_return() {
    // Unrescued host error inside the ensure body: the raise wins
    // over the pending return (CRuby), surfacing to the outer
    // rescue.
    let mut rt = rt_with_flaky_host_fn();
    let v = rt
        .eval(
            r#"
            def transfer
              return :sent
            ensure
              host_close(true)
            end
            begin
              transfer
              :not_here
            rescue IOError
              :escaped
            end
            "#,
            "t.rb",
        )
        .expect("escaping host trap must be rescuable outside");
    assert_sym(&mut rt, &v, "escaped");
}

#[test]
fn host_trap_rescued_inside_break_ensure_resumes_break() {
    let mut rt = rt_with_flaky_host_fn();
    let v = rt
        .eval(
            r#"
            while true
              begin
                break :broke
              ensure
                begin
                  host_close(true)
                rescue IOError
                end
              end
            end
            "#,
            "t.rb",
        )
        .expect("contained host trap must not surface");
    assert_sym(&mut rt, &v, "broke");
}

#[test]
fn host_trap_rescued_during_exception_path_ensure_keeps_original() {
    // Exception already unwinding enters the ensure; a host trap
    // fired + rescued inside the body must not replace the original
    // exception at the body's tail.
    let mut rt = rt_with_flaky_host_fn();
    let v = rt
        .eval(
            r#"
            def transfer
              raise ArgumentError, "original"
            ensure
              begin
                host_close(true)
              rescue IOError
              end
            end
            begin
              transfer
              :not_here
            rescue ArgumentError
              :kept_original
            rescue IOError
              :wrong_exception
            end
            "#,
            "t.rb",
        )
        .expect("original exception must stay rescuable");
    assert_sym(&mut rt, &v, "kept_original");
}

#[test]
fn host_trap_contained_in_block_break_ensure() {
    // Block-`break` walking the yielding method's ensure while the
    // cleanup swallows a host error.
    let mut rt = rt_with_flaky_host_fn();
    let v = rt
        .eval(
            r#"
            def session
              yield
            ensure
              begin
                host_close(true)
              rescue IOError
              end
            end
            session { break :early }
            "#,
            "t.rb",
        )
        .expect("contained host trap must not surface");
    assert_sym(&mut rt, &v, "early");
}
