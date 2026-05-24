// Error and span types. The Span flow is wired all the way through (Spanned<Expr>
// in `ast`, `op_spans` on Proto), but the panic→Trap migration happens in P0-B-2.
// These items are referenced by `bytecode` and `compiler` indirectly via Span;
// the rest will become live once panics are rewritten to throw a Trap.
#![allow(dead_code)]

use std::rc::Rc;

/// Source position. Byte offset is what Prism gives us cheaply; line/column
/// are resolved lazily at display time against the original source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub(crate) byte_offset: u32,
}

impl Span {
    pub(crate) fn at(byte_offset: usize) -> Self {
        Span { byte_offset: byte_offset as u32 }
    }
    pub(crate) const ZERO: Span = Span { byte_offset: 0 };
}

/// Resolve a byte offset to (1-based line, 1-based column) by scanning the
/// source. Slow but only called on the error path.
pub(crate) fn line_col(source: &str, byte_offset: u32) -> (u32, u32) {
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, ch) in source.char_indices() {
        if i as u32 >= byte_offset { break; }
        if ch == '\n' { line += 1; col = 1; } else { col += 1; }
    }
    (line, col)
}

/// A Ruby-visible error. Today this is the closed set rubyrs can produce;
/// we'll grow it to a class hierarchy with the rescue-by-class feature.
#[derive(Debug)]
pub enum RubyError {
    SyntaxError { msg: String },
    NoMethodError { method: String, recv_type: &'static str },
    ArgumentError { msg: String },
    TypeError { msg: String },
    RuntimeError { msg: String },
    NameError { msg: String },
}

impl RubyError {
    pub(crate) fn class_name(&self) -> &'static str {
        match self {
            RubyError::SyntaxError { .. } => "SyntaxError",
            RubyError::NoMethodError { .. } => "NoMethodError",
            RubyError::ArgumentError { .. } => "ArgumentError",
            RubyError::TypeError { .. } => "TypeError",
            RubyError::RuntimeError { .. } => "RuntimeError",
            RubyError::NameError { .. } => "NameError",
        }
    }
    pub(crate) fn message(&self) -> String {
        match self {
            RubyError::SyntaxError { msg }
            | RubyError::ArgumentError { msg }
            | RubyError::TypeError { msg }
            | RubyError::RuntimeError { msg }
            | RubyError::NameError { msg } => msg.clone(),
            RubyError::NoMethodError { method, recv_type } => {
                format!("undefined method `{}' for {}", method, recv_type)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrapFrame {
    pub filename: Rc<str>,
    pub method: Rc<str>,
    pub span: Span,
}

/// A unwinding error. Carries the Ruby-visible cause and the call-stack
/// snapshot at the throw site.
#[derive(Debug)]
pub struct Trap {
    pub err: RubyError,
    pub backtrace: Vec<TrapFrame>,
}

impl Trap {
    /// Convenience constructor for host fns that want to raise an error
    /// with no backtrace; the dispatch loop fills the backtrace from the
    /// caller's frames.
    pub fn new(err: RubyError) -> Self {
        Trap { err, backtrace: vec![] }
    }
}
