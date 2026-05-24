//! Embedding example.
//!
//! Demonstrates the three core capabilities of the rubyrs host API:
//! - registering host functions callable from Ruby
//! - capturing puts/print output into a Rust buffer
//! - persisting class/method definitions across multiple eval calls
//!
//! Run with: `cargo run --release --example embed`

use std::cell::RefCell;
use std::rc::Rc;

use rubyrs::{Runtime, Value};

fn main() {
    // ------------------------------------------------------------------
    // 1. Register a host function. The closure takes evaluated Value
    //    arguments and returns a Value (or a Trap).
    // ------------------------------------------------------------------

    let mut rt = Runtime::new();

    rt.register_fn("host_pid", |_args| {
        Ok(Value::Int(std::process::id() as i64))
    });

    rt.register_fn("host_double", |args| {
        if let [Value::Int(n)] = args {
            Ok(Value::Int(n * 2))
        } else {
            // We'd normally return a TypeError Trap; for the example we
            // just bail with nil.
            Ok(Value::Nil)
        }
    });

    rt.eval(
        r#"
        puts "process id: #{host_pid}"
        puts "21 doubled is #{host_double(21)}"
        "#,
        "example1",
    ).unwrap();

    // ------------------------------------------------------------------
    // 2. Capture stdout into a buffer. Useful for testing or for hosting
    //    Ruby DSLs whose output you want to redirect elsewhere.
    // ------------------------------------------------------------------

    // SharedBuf lets the host read what the script wrote without
    // dismantling the Box<dyn Write> first.
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));

    rt.eval(r#"puts "captured: #{1 + 2}""#, "example2").unwrap();

    let captured = buf.take();
    println!(
        "host saw {} bytes from the script: {}",
        captured.len(),
        String::from_utf8_lossy(&captured).trim()
    );

    // ------------------------------------------------------------------
    // 3. State persists across eval calls. Restore real stdout, then
    //    define a class in one eval and use it in another.
    // ------------------------------------------------------------------

    rt.set_stdout(Box::new(std::io::stdout()));

    rt.eval(
        r#"
        class Counter
          def initialize(start)
            @count = start
          end
          def inc
            @count = @count + 1
          end
          def value
            @count
          end
        end
        "#,
        "define_counter",
    ).unwrap();

    rt.eval(
        r#"
        c = Counter.new(10)
        c.inc
        c.inc
        c.inc
        puts "counter after 3 incs: #{c.value}"
        "#,
        "use_counter",
    ).unwrap();
}

/// A `Write` sink whose written bytes can be inspected from the host. We
/// hand a clone to the runtime via `set_stdout`; the host keeps the
/// original to read from.
#[derive(Clone)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl SharedBuf {
    fn new() -> Self { SharedBuf(Rc::new(RefCell::new(Vec::new()))) }
    fn take(&self) -> Vec<u8> { std::mem::take(&mut *self.0.borrow_mut()) }
}

impl std::io::Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}
