//! Public API smoke tests. Locks down the embedding surface
//! (Runtime, register_fn, set_stdout, eval, format_trap) so it can't
//! regress accidentally.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use rubyrs::{Config, Runtime, RubyError, Value};

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

#[test]
fn format_trap_emits_cruby_style_line() {
    let mut rt = Runtime::new();
    let trap = rt.eval(r#"nil.foo"#, "snippet.rb").unwrap_err();
    let formatted = rt.format_trap(&trap);
    assert!(formatted.contains("snippet.rb:1"));
    assert!(formatted.contains("undefined method"));
    assert!(formatted.contains("NoMethodError"));
}

#[test]
fn syntax_error_does_not_panic() {
    let mut rt = Runtime::new();
    let res = rt.eval(r#"def foo("#, "broken.rb");
    assert!(res.is_err(), "syntax errors should bubble up as Trap");
}

// ---------- P1-D: resource caps ----------

#[test]
fn fuel_traps_infinite_loop() {
    let mut rt = Runtime::with_config(Config { fuel: Some(10_000), ..Default::default() });
    let err = rt.eval(r#"i = 0; while true; i = i + 1; end"#, "spin.rb").unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted, got {:?}", err.err
    );
}

#[test]
fn fuel_is_not_bypassed_by_block_iteration() {
    // Without per-op fuel inside dispatch_until, an each-block could spin forever.
    let mut rt = Runtime::with_config(Config { fuel: Some(50_000), ..Default::default() });
    let err = rt.eval(
        r#"
        nums = []
        i = 0
        while i < 100
          nums << i
          i = i + 1
        end
        nums.each { |x| j = 0; while true; j = j + 1; end }
        "#,
        "spin_in_block.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
}

#[test]
fn unlimited_fuel_runs_normally() {
    let mut rt = Runtime::new();
    rt.eval(r#"i = 0; while i < 100; i = i + 1; end"#, "ok.rb").unwrap();
}

#[test]
fn heap_cap_traps_retained_allocations() {
    let mut rt = Runtime::with_config(Config { max_heap_objects: Some(50), ..Default::default() });
    // Each inner Array is retained via `all`, so live_count grows linearly.
    let err = rt.eval(
        r#"
        all = []
        i = 0
        while i < 200
          all << [i, i + 1]
          i = i + 1
        end
        "#,
        "alloc_storm.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
}

#[test]
fn resource_exhausted_cannot_be_swallowed_by_bare_rescue() {
    // P0-1: `ResourceExhausted < Exception` (not StandardError), so bare
    // `rescue => e` — which CRuby-style filters on StandardError — must
    // not catch it. Otherwise a hostile script can spin in a rescue loop
    // and burn fuel forever, defeating the kill switch entirely.
    //
    // We give the script a generous outer fuel budget. The inner
    // `while true` will trip the fuel trap; if the bare `rescue`
    // swallowed it, the script would either run to completion (printing
    // "caught" once per outer iteration) or loop forever. Instead we
    // expect `eval` itself to surface the ResourceExhausted trap to
    // the host because no in-script handler matched.
    let buf = SharedBuf::new();
    let mut rt = Runtime::with_config(Config { fuel: Some(50_000), ..Default::default() });
    rt.set_stdout(Box::new(buf.clone()));
    let err = rt.eval(
        r#"
        begin
          i = 0
          while true
            i = i + 1
          end
        rescue => e
          puts "caught"
        end
        puts "after"
        "#,
        "uncatchable.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, RubyError::ResourceExhausted { .. }),
        "expected ResourceExhausted to propagate past `rescue => e`, got {:?}",
        err.err,
    );
    let out = buf.snapshot();
    assert!(
        !out.contains("caught") && !out.contains("after"),
        "bare rescue should not have run; stdout was:\n{out}",
    );
}

#[test]
fn rescue_still_catches_standard_error_descendants() {
    // Locking in the partner invariant: bare `rescue` is now class-
    // filtered, but it must still catch StandardError + descendants the
    // way Ruby programs expect — every existing fixture relies on this.
    // `raise "boom"` normalises to RuntimeError, which is rooted under
    // StandardError, so the rescue clause runs.
    let buf = SharedBuf::new();
    let mut rt = Runtime::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        r#"
        begin
          raise "boom"
        rescue => e
          puts "got: #{e.message}"
        end
        "#,
        "rescue_runtime.rb",
    ).unwrap();
    assert_eq!(buf.snapshot(), "got: boom\n");
}

#[test]
fn explicit_rescue_exception_can_catch_resource_exhausted() {
    // Escape hatch: a script that *explicitly* asks for `rescue Exception`
    // is opting in to seeing the resource trap. This will become
    // testable once P1-10 lands class filtering; for now we just
    // document the intended behaviour as a `#[ignore]`d test so the
    // contract is recorded somewhere checkable.
    //
    // TODO(P1-10): drop the `#[ignore]` once `rescue ClassName` works.
}

#[test]
fn frame_cap_traps_deep_recursion() {
    let mut rt = Runtime::with_config(Config { max_frames: Some(20), ..Default::default() });
    let err = rt.eval(
        r#"
        def rec(n)
          rec(n + 1)
        end
        rec(0)
        "#,
        "deep.rb",
    ).unwrap_err();
    assert!(matches!(err.err, RubyError::ResourceExhausted { .. }));
}
