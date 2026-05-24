//! rubyrs — a tiny Ruby-subset runtime, embeddable in Rust hosts.
//!
//! # Quick start
//!
//! ```no_run
//! use rubyrs::{Runtime, Value};
//!
//! let mut rt = Runtime::new();
//! rt.eval(r#"puts "hello, world""#, "inline").unwrap();
//!
//! // Register a host function callable from Ruby:
//! rt.register_fn("host_pid", |_args| Ok(Value::Int(std::process::id() as i64)));
//! rt.eval(r#"puts "pid is #{host_pid}""#, "inline").unwrap();
//! ```
//!
//! See [`docs/SUBSET.md`](https://github.com/linyiru/rubyrs/blob/master/docs/SUBSET.md)
//! for the Ruby semantics this runtime does and does not support.

mod ast;
mod bytecode;
mod compiler;
mod error;
mod heap;
mod intern;
mod value;
mod vm;

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;

pub use error::{RubyError, Span, Trap, TrapFrame};
pub use value::Value;
pub use intern::SymId;

/// Configuration for a [`Runtime`]. Defaults are unlimited; tighten for
/// untrusted scripts.
#[derive(Default)]
pub struct Config {
    /// When true, every potential GC point triggers a full collection.
    /// Useful for catching root-set bugs in host code; rough on
    /// performance. Equivalent to `STRESS_GC=1` env var.
    pub stress_gc: bool,
    /// If `Some(n)`, dispatching more than `n` ops returns a
    /// `ResourceExhausted` trap. Includes ops inside blocks via
    /// `dispatch_until`, so a runaway `[1].each { while true ... }`
    /// cannot bypass the limit.
    pub fuel: Option<u64>,
    /// If `Some(n)`, allocating past `n` simultaneously-live heap
    /// objects (Instance / Array / Hash) returns a `ResourceExhausted`
    /// trap. Checked after `maybe_gc`, so only steady-state allocation
    /// counts.
    pub max_heap_objects: Option<usize>,
    /// If `Some(n)`, pushing past `n` simultaneously-live frames
    /// returns a `ResourceExhausted` trap before the host's Rust stack
    /// can overflow.
    pub max_frames: Option<usize>,
}

/// A self-contained rubyrs runtime. State (class definitions, top-level
/// methods, registered host functions, GC heap) persists across calls to
/// [`Runtime::eval`].
pub struct Runtime {
    vm: vm::Vm,
    /// Source text per filename, retained so that backtrace formatting can
    /// resolve byte offsets to line/column without re-reading the file.
    sources: HashMap<Rc<str>, Rc<str>>,
    /// Per-call-site inline-cache id allocator, monotonically increasing
    /// across every `eval` call so subsequent compiles don't collide with
    /// cached methods from earlier ones.
    cache_counter: u32,
}

impl Runtime {
    pub fn new() -> Self {
        Self::with_config(Config::default())
    }

    pub fn with_config(cfg: Config) -> Self {
        let interner = intern::Interner::new();
        let mut vm = vm::Vm::new(vec![], interner);
        if cfg.stress_gc { vm.stress_gc = true; }
        vm.fuel = cfg.fuel;
        vm.max_frames = cfg.max_frames;
        vm.heap.max_live = cfg.max_heap_objects;
        let mut rt = Runtime { vm, sources: HashMap::new(), cache_counter: 0 };
        rt.load_preamble();
        rt
    }

    /// Bootstrap the built-in Ruby class hierarchy (currently just
    /// exceptions) by `eval`-ing a small Ruby preamble. Done with the
    /// runtime's own machinery so the resulting classes look identical
    /// to user-defined ones (no special-cased C structs).
    fn load_preamble(&mut self) {
        const PREAMBLE: &str = r#"
class Exception
  def initialize(msg)
    @message = msg
  end
  def message
    @message
  end
  def to_s
    @message
  end
end
class StandardError < Exception
end
class RuntimeError < StandardError
end
class NoMethodError < StandardError
end
class ArgumentError < StandardError
end
class TypeError < StandardError
end
class NameError < StandardError
end
class ResourceExhausted < StandardError
end
"#;
        self.eval(PREAMBLE, "<rubyrs:preamble>")
            .expect("ICE: failed to load built-in exception preamble");
    }

    /// Replace the runtime's stdout sink. Lets a host capture `puts` /
    /// `print` output (e.g. into a `Vec<u8>` buffer) instead of having
    /// it go to the process stdout.
    pub fn set_stdout(&mut self, w: Box<dyn Write>) {
        self.vm.stdout = w;
    }

    /// Register a host function callable from Ruby code with `name(args)`.
    /// The function receives evaluated argument values and returns either
    /// a `Value` or a `Trap`. Calling `register_fn` with the same name
    /// replaces a previous registration.
    pub fn register_fn<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&[Value]) -> Result<Value, Trap> + 'static,
    {
        let id = self.vm.interner.intern(name);
        self.vm.host_fns.insert(id, Rc::new(f));
    }

    /// Parse, compile, and run a Ruby source. The returned value is the
    /// final expression of the script; embedders can ignore it for
    /// statements with no return value.
    pub fn eval(&mut self, source: &str, filename: &str) -> Result<Value, Trap> {
        let filename_rc: Rc<str> = Rc::from(filename);
        self.sources.insert(filename_rc.clone(), Rc::from(source));

        let parse_result = ruby_prism::parse(source.as_bytes());
        let errors: Vec<_> = parse_result.errors().collect();
        if !errors.is_empty() {
            let msg = errors.iter()
                .map(|e| format!("{:?}", e))
                .collect::<Vec<_>>().join("; ");
            return Err(Trap {
                err: RubyError::SyntaxError { msg },
                backtrace: vec![],
            });
        }
        let prog = ast::tr(&parse_result.node());
        let entry = compiler::compile_proto(
            "<main>".into(), vec![], &[prog], filename_rc,
            &mut self.vm.protos, &mut self.vm.interner, &mut self.cache_counter,
        );
        self.vm.ensure_call_caches(self.cache_counter as usize);
        self.vm.run(entry)
    }

    pub fn eval_file(&mut self, path: &Path) -> Result<Value, Trap> {
        let source = std::fs::read_to_string(path).map_err(|e| Trap {
            err: RubyError::SyntaxError {
                msg: format!("cannot read {}: {}", path.display(), e),
            },
            backtrace: vec![],
        })?;
        let filename = path.to_string_lossy().into_owned();
        self.eval(&source, &filename)
    }

    /// Format a [`Trap`] CRuby-style:
    /// `file:line:in 'method': msg (Class)`, with one `\tfrom ...` line
    /// per remaining backtrace frame.
    ///
    /// Uses the source texts retained from prior `eval` calls to resolve
    /// byte offsets into line numbers.
    pub fn format_trap(&self, trap: &Trap) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let frames = &trap.backtrace;
        let cls = trap.err.class_name();
        let msg = trap.err.message();
        if let Some(top) = frames.first() {
            let line = self.line_for(&top.filename, top.span.byte_offset);
            let _ = writeln!(out, "{}:{}:in `{}': {} ({})", top.filename, line, top.method, msg, cls);
            for f in frames.iter().skip(1) {
                let line = self.line_for(&f.filename, f.span.byte_offset);
                let _ = writeln!(out, "\tfrom {}:{}:in `{}'", f.filename, line, f.method);
            }
        } else {
            let _ = writeln!(out, "rubyrs: {} ({})", msg, cls);
        }
        out
    }

    fn line_for(&self, filename: &str, byte_offset: u32) -> u32 {
        match self.sources.get(filename) {
            Some(src) => error::line_col(src, byte_offset).0,
            None => 0,
        }
    }

    /// Resolve a `SymId` back to its string representation.
    pub fn resolve_sym(&self, sym: SymId) -> &str {
        self.vm.interner.resolve(sym)
    }

    /// Unpack a `Value::Array` into a Rust `Vec<Value>` by cloning elements.
    /// Returns `None` if the value is not an Array.
    pub fn resolve_array(&self, val: &Value) -> Option<Vec<Value>> {
        if let Value::Array(id) = val {
            Some(self.vm.heap.array(*id).clone())
        } else {
            None
        }
    }

    /// Unpack a `Value::Hash` into a Rust `Vec<(Value, Value)>` by cloning.
    /// Returns `None` if the value is not a Hash.
    pub fn resolve_hash(&self, val: &Value) -> Option<Vec<(Value, Value)>> {
        if let Value::Hash(id) = val {
            Some(self.vm.heap.hash(*id).clone())
        } else {
            None
        }
    }
}

impl Default for Runtime {
    fn default() -> Self { Self::new() }
}
