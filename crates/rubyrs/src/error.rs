// Error and span types. The Span flow is wired all the way through
// (Spanned<Expr> in `ast`, `op_spans` on Proto); the panic→Trap
// migration that introduced the original `#![allow(dead_code)]`
// completed in P0-B-2 and the items are all live now.

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

/// Format Prism parse diagnostics into a SyntaxError message body.
/// Each diagnostic becomes `"L<line>:<col>: <message>"`, joined with
/// `"; "` when there are multiple.
///
/// `ruby_prism::Diagnostic` derives `Debug`, but its Debug impl
/// stringifies the internal `NonNull<pm_diagnostic_t>` /
/// `NonNull<pm_parser_t>` pointer fields as `0xADDR` plus a
/// `PhantomData<...>` marker. Using that for the user-facing
/// SyntaxError message leaked raw pointers into rubyrs output —
/// e.g. `Diagnostic { diag: 0x153370, parser: 0x1358e0, marker:
/// PhantomData<&...pm_diagnostic_t> }`. The published API
/// (`message()` + `location().start_offset()`) is what should be
/// formatted instead.
pub(crate) fn format_prism_errors<'a>(
    source: &str,
    errors: impl Iterator<Item = ruby_prism::Diagnostic<'a>>,
) -> String {
    errors
        .map(|e| {
            let (line, col) = line_col(source, e.location().start_offset() as u32);
            format!("L{line}:{col}: {}", e.message())
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A Ruby-visible error. Today this is the closed set rubyrs can produce;
/// we'll grow it to a class hierarchy with the rescue-by-class feature.
#[derive(Debug)]
pub enum RubyError {
    SyntaxError { msg: String },
    NoMethodError { method: String, recv_type: std::borrow::Cow<'static, str> },
    ArgumentError { msg: String },
    TypeError { msg: String },
    RuntimeError { msg: String },
    NameError { msg: String },
    /// `Hash#fetch(key)` with no default and no block, when the
    /// key isn't present. Routed through `unwind_with_exception`
    /// by `dispatch`, so a script `rescue KeyError => e` catches
    /// it like CRuby.
    KeyError { msg: String },
    /// Out-of-range indexing into a String (via `s[i] = x` and
    /// friends). The Array#[] / Array#[]= path returns nil for
    /// OOB rather than raising, matching CRuby. Arrays grow on
    /// write; strings don't (at least not without a different
    /// CRuby method), hence the trap.
    IndexError { msg: String },
    /// Mutating a frozen String (`freeze`d or interned). CRuby's
    /// FrozenError; rescued by `rescue FrozenError`.
    FrozenError { msg: String },
    /// `break` from a stored Proc (e.g. a Hash default-block)
    /// instead of an actively-yielded-to iterator block. CRuby
    /// raises LocalJumpError because there's no loop body to
    /// break out of. Rescued by `rescue LocalJumpError`.
    LocalJumpError { msg: String },
    /// Integer `/` or `%` with a zero divisor. CRuby raises
    /// `ZeroDivisionError`; without this variant the Rust
    /// `i64::div` would panic the host process. Float `/ 0.0`
    /// is NOT an error in CRuby (returns `±Infinity` or `NaN`)
    /// and remains so here — only the Int×Int path traps.
    ZeroDivisionError { msg: String },
    /// Value out of an expected range. CRuby's `RangeError` —
    /// raised by e.g. `Integer#chr` on bytes outside `0..255`,
    /// `Numeric#step` on negative step with no end, and
    /// `Integer#pow(exp, mod)` when the exponent is negative
    /// (the modular inverse may not exist; we don't compute it).
    /// Rescued by `rescue RangeError`.
    RangeError { msg: String },
    /// IEEE-754 special value (NaN / ±Infinity) where an Integer-
    /// range value was expected. CRuby's `FloatDomainError`,
    /// which sits under `RangeError` — so `rescue RangeError`
    /// or a bare `rescue` still catches it; users who want to
    /// distinguish float-vs-other domain failures can write
    /// `rescue FloatDomainError`. Raised by e.g. divmod with a
    /// NaN divisor, `Float::INFINITY.to_i`, `Float::NAN.to_i`.
    FloatDomainError { msg: String },
    /// Resource limits exceeded (fuel, heap, stack depth). Used by P1-D
    /// when a Runtime was configured with caps for untrusted scripts.
    ResourceExhausted { msg: String },
    /// Filesystem I/O blocked by the
    /// `Config::allow_filesystem_io: false` sandbox cap. Raised by
    /// `Vm::check_filesystem_io_allowed` from every File.* class
    /// method, by `__dir__` (when canonicalize is gated), and by
    /// anything else that touches the host filesystem outside the
    /// `require` family. Rescuable via `rescue IOError`.
    IOError { msg: String },
    /// `require` / `require_relative` / `cext_require` blocked by
    /// the FS sandbox cap. Distinct from `IOError` so scripts can
    /// `rescue LoadError` for "feature unavailable" without
    /// catching every File.* failure. Raised by
    /// `Vm::check_load_allowed`.
    LoadError { msg: String },
    /// A Ruby-level `raise` whose exception class wasn't caught by any
    /// `rescue` clause on the call stack. Carries the script's class
    /// name and message so the host can log/format whatever it likes,
    /// then decide to retry, abort, or continue. Before this variant,
    /// the VM called `std::process::exit(1)` directly — a fatal action
    /// the host couldn't recover from. See
    /// [docs/SECURITY.md § known attack surface](../docs/SECURITY.md).
    Uncaught { class_name: String, message: String },
}

/// Built-in exception hierarchy parent table — `(child, parent)`.
/// Mirrors `crates/rubyrs/src/preamble/exceptions.rb` (the
/// runtime's actual source of truth). Used by
/// [`RubyError::is_a`] for hierarchy walks without needing the
/// live `Runtime` class table.
///
/// Maintenance note: when adding a built-in exception class to
/// `preamble/exceptions.rb`, add the matching `(child, parent)`
/// row here. The chain is walked iteratively, so leaves and
/// intermediate nodes both belong here — only the root
/// "Exception" is implicit (it has no parent and acts as the
/// loop terminator). `tests/embed.rs::is_a_*` tests cover the
/// expected chains and lock against drift.
const BUILTIN_EXCEPTION_PARENT: &[(&str, &str)] = &[
    ("StandardError", "Exception"),
    ("RuntimeError", "StandardError"),
    ("NoMethodError", "StandardError"),
    ("ArgumentError", "StandardError"),
    ("TypeError", "StandardError"),
    ("NameError", "StandardError"),
    ("ScriptError", "Exception"),
    ("NotImplementedError", "ScriptError"),
    ("IndexError", "StandardError"),
    ("KeyError", "IndexError"),
    ("ZeroDivisionError", "StandardError"),
    ("RangeError", "StandardError"),
    ("FloatDomainError", "RangeError"),
    ("LocalJumpError", "StandardError"),
    ("FrozenError", "RuntimeError"),
    // Deliberately `< Exception`, NOT `< StandardError` — see
    // ADR 0008: hosts must not be able to swallow their own
    // resource trap via a bare `rescue` clause.
    ("ResourceExhausted", "Exception"),
    ("IOError", "StandardError"),
    ("LoadError", "ScriptError"),
];

impl RubyError {
    /// Does this error correspond to the given Ruby exception class
    /// name? Handles both the direct host-side variant
    /// (`RubyError::NoMethodError { .. }`) and the script-raised
    /// wrapped form (`RubyError::Uncaught { class_name, .. }`) —
    /// both surface as the same Ruby-level class to the script
    /// author, so embedding hosts should normally treat them the
    /// same way.
    ///
    /// Before this helper, tests and host code had to write
    ///
    /// ```ignore
    /// match err.err {
    ///     RubyError::NoMethodError { .. } => { /* ok */ }
    ///     RubyError::Uncaught { class_name, .. } if class_name == "NoMethodError" => { /* ok */ }
    ///     other => panic!("expected NoMethodError, got {other:?}"),
    /// }
    /// ```
    ///
    /// which is now just `err.err.is("NoMethodError")`.
    ///
    /// Note: comparison is exact, case-sensitive, and on the
    /// *bare* class name — passing `"StandardError"` won't match
    /// a `RuntimeError` even though RuntimeError is a descendant
    /// in CRuby's class hierarchy. For hierarchy-aware matching
    /// (`rescue StandardError => e` shape), use [`Self::is_a`]
    /// instead.
    pub fn is(&self, class_name: &str) -> bool {
        match self {
            RubyError::Uncaught { class_name: cn, .. } => cn == class_name,
            other => other.class_name() == class_name,
        }
    }

    /// Hierarchy-aware variant of [`Self::is`]. Returns `true` if
    /// the error's Ruby-level class equals `class_name` OR is a
    /// descendant of it per the built-in exception hierarchy
    /// (mirrors `crates/rubyrs/src/preamble/exceptions.rb`, the
    /// runtime's actual source of truth).
    ///
    /// ```ignore
    /// // All true:
    /// err_from_RuntimeError.is_a("RuntimeError");
    /// err_from_RuntimeError.is_a("StandardError");
    /// err_from_RuntimeError.is_a("Exception");
    /// err_from_KeyError.is_a("IndexError");      // KeyError < IndexError
    /// err_from_FrozenError.is_a("RuntimeError"); // FrozenError < RuntimeError
    /// // False:
    /// err_from_RuntimeError.is_a("ScriptError"); // different branch
    /// err_from_ResourceExhausted.is_a("StandardError"); // deliberately < Exception, not StandardError
    /// ```
    ///
    /// User-defined subclasses (`class MyError < StandardError`
    /// inside a script that then `raise`s) are NOT in the static
    /// table — for them this method falls back to the same exact
    /// match `is` does. If `Uncaught.class_name` happens to BE a
    /// known built-in (the script said `raise RuntimeError, "..."`),
    /// the walk works because the chain starts at a known node.
    /// Hosts that need full hierarchy walk on arbitrary script-
    /// defined classes should query the live `Runtime` class
    /// table directly via the embedding API.
    pub fn is_a(&self, class_name: &str) -> bool {
        let start = match self {
            RubyError::Uncaught { class_name: cn, .. } => cn.as_str(),
            other => other.class_name(),
        };
        let mut cur = start;
        loop {
            if cur == class_name {
                return true;
            }
            match BUILTIN_EXCEPTION_PARENT
                .iter()
                .find(|(child, _)| *child == cur)
            {
                Some((_, parent)) => cur = parent,
                None => return false,
            }
        }
    }

    pub(crate) fn class_name(&self) -> &'static str {
        match self {
            RubyError::SyntaxError { .. } => "SyntaxError",
            RubyError::NoMethodError { .. } => "NoMethodError",
            RubyError::ArgumentError { .. } => "ArgumentError",
            RubyError::TypeError { .. } => "TypeError",
            RubyError::RuntimeError { .. } => "RuntimeError",
            RubyError::NameError { .. } => "NameError",
            RubyError::KeyError { .. } => "KeyError",
            RubyError::IndexError { .. } => "IndexError",
            RubyError::FrozenError { .. } => "FrozenError",
            RubyError::LocalJumpError { .. } => "LocalJumpError",
            RubyError::ZeroDivisionError { .. } => "ZeroDivisionError",
            RubyError::RangeError { .. } => "RangeError",
            RubyError::FloatDomainError { .. } => "FloatDomainError",
            RubyError::ResourceExhausted { .. } => "ResourceExhausted",
            RubyError::IOError { .. } => "IOError",
            RubyError::LoadError { .. } => "LoadError",
            // Uncaught carries the actual class name from the script's
            // exception object; static-class machinery doesn't apply.
            // Hosts that want the Ruby-level class name should pattern-
            // match on `Uncaught { class_name, .. }` directly.
            RubyError::Uncaught { .. } => "Uncaught",
        }
    }
    pub(crate) fn message(&self) -> String {
        match self {
            RubyError::SyntaxError { msg }
            | RubyError::ArgumentError { msg }
            | RubyError::TypeError { msg }
            | RubyError::RuntimeError { msg }
            | RubyError::NameError { msg }
            | RubyError::KeyError { msg }
            | RubyError::IndexError { msg }
            | RubyError::FrozenError { msg }
            | RubyError::LocalJumpError { msg }
            | RubyError::ZeroDivisionError { msg }
            | RubyError::RangeError { msg }
            | RubyError::FloatDomainError { msg }
            | RubyError::ResourceExhausted { msg }
            | RubyError::IOError { msg }
            | RubyError::LoadError { msg } => msg.clone(),
            RubyError::Uncaught { message, .. } => message.clone(),
            RubyError::NoMethodError { method, recv_type } => {
                // Visibility-error call sites
                // (dispatch.rs:1475/1497) store a full-sentence
                // form like "private method 'lookup' called" /
                // "protected method 'foo' called" in `method`,
                // because CRuby's error message uses a different
                // shape for those cases ("private method 'X'
                // called for <recv>" — not "undefined method
                // 'X' for <recv>"). Detect that shape and skip
                // the "undefined method" wrap. The missing-
                // method form still uses the standard wrap.
                // (TRY_RUNS pass-10 layer #5.)
                if method.starts_with("private method ")
                    || method.starts_with("protected method ")
                {
                    format!("{} for {}", method, recv_type)
                } else {
                    format!("undefined method `{}' for {}", method, recv_type)
                }
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
