//! Public API smoke tests. Locks down the embedding surface
//! (Runtime, register_fn, set_stdout, eval, format_trap) so it can't
//! regress accidentally.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use rubyrs::{Runtime, Value};

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
