//! Output sink abstraction — the no_std-compatible Tier 1
//! trait that `Vm::stdout` uses, decoupled from
//! `std::io::Write`.
//!
//! Why this exists: ADR 0018 Phase 1 (`rubyrs-core` extraction)
//! adds `#![no_std]` to the core crate. `std::io::Write` is not
//! available in `no_std`. This module defines a minimal byte-
//! oriented sink trait that lives in core, plus a `StdSink`
//! adapter that wraps any `std::io::Write` so the public
//! `Runtime::set_stdout(Box<dyn std::io::Write>)` embed API
//! stays backward-compatible.
//!
//! See [STD_AUDIT.md](../../docs/STD_AUDIT.md) Open Question
//! #1 for the design rationale (vendor a minimal trait vs
//! depend on a no_std-compatible IO crate — we vendor).

use alloc::boxed::Box;
use alloc::string::String;
use core::fmt;

/// Minimal byte-oriented output sink for the VM's stdout.
///
/// Implementors handle two kinds of writes:
/// - `write_bytes(buf)` — the canonical bytes path
/// - `write_fmt(args)` — what the `write!` / `writeln!` macros
///   ultimately dispatch to. Default impl funnels into
///   `write_bytes` via a `core::fmt::Write` adapter, so most
///   implementors only override `write_bytes` + optionally
///   `flush`.
///
/// Errors trap the script as `IOError` (or a registered cext
/// equivalent). The trait owns an `OutputError` for that
/// purpose rather than re-using `std::io::Error` (which
/// doesn't exist in `no_std`).
pub trait OutputSink {
    /// Write all bytes or return `Err`. Implementations MUST
    /// not perform partial writes — the VM relies on
    /// "wrote everything or trapped".
    fn write_bytes(&mut self, buf: &[u8]) -> Result<(), OutputError>;

    /// Flush. Default: no-op. Override for buffered sinks.
    fn flush(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    /// `write!` / `writeln!` macro support. Default
    /// implementation routes through a `core::fmt::Write`
    /// adapter into `write_bytes` — implementors rarely need
    /// to override this.
    ///
    /// `fmt::Error` from the adapter is converted to
    /// `OutputError`; if the underlying `write_bytes` errored,
    /// the captured error is returned (the adapter halts
    /// formatting on first error).
    fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<(), OutputError> {
        struct Adapter<'a, S: ?Sized> {
            sink: &'a mut S,
            err: Option<OutputError>,
        }
        impl<S: OutputSink + ?Sized> fmt::Write for Adapter<'_, S> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                if let Err(e) = self.sink.write_bytes(s.as_bytes()) {
                    self.err = Some(e);
                    return Err(fmt::Error);
                }
                Ok(())
            }
        }
        let mut adapter = Adapter { sink: self, err: None };
        let fmt_result = fmt::Write::write_fmt(&mut adapter, args);
        if let Some(e) = adapter.err {
            return Err(e);
        }
        // No captured byte-write error → any fmt::Error came
        // from the formatter itself (e.g. a Display impl that
        // returned Err). Treat as a generic format failure.
        fmt_result.map_err(|_| OutputError::new("format error"))
    }
}

/// Errors from `OutputSink` ops. Owned `String` keeps the
/// type `no_std`-compatible (alloc::string::String works
/// in `no_std + alloc`).
#[derive(Debug)]
pub struct OutputError {
    msg: String,
}

impl OutputError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    pub fn message(&self) -> &str {
        &self.msg
    }
}

impl fmt::Display for OutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

/// Default sink — discards all writes (the Tier 1 default
/// per ADR 0017: "no script output sink by default; embedders
/// opt in by calling `Runtime::set_stdout`"). Used as
/// `Vm::stdout`'s value before `set_stdout` is called.
///
/// Equivalent to `/dev/null` — bytes go in, nothing comes
/// back out. No allocation; no syscall.
#[derive(Debug, Default)]
pub struct NullSink;

impl OutputSink for NullSink {
    fn write_bytes(&mut self, _buf: &[u8]) -> Result<(), OutputError> {
        Ok(())
    }
}

/// Adapter wrapping any `std::io::Write` as an `OutputSink`.
/// Lives in this module behind `#[cfg(feature = "std-sink")]`
/// (today: always-on since `rubyrs-core` doesn't exist yet)
/// so the `rubyrs-core` crate Phase 1 produces stays
/// `no_std`-clean while the `rubyrs` facade keeps its
/// `set_stdout(Box<dyn std::io::Write>)` public API.
///
/// Constructed by `Runtime::set_stdout`'s internal wrapper —
/// embedders never need to mention `StdSink` directly.
#[cfg(feature = "std-sink")]
pub struct StdSink<W: std::io::Write>(pub W);

#[cfg(feature = "std-sink")]
impl<W: std::io::Write> OutputSink for StdSink<W> {
    fn write_bytes(&mut self, buf: &[u8]) -> Result<(), OutputError> {
        std::io::Write::write_all(&mut self.0, buf)
            .map_err(|e| OutputError::new(alloc::format!("io: {e}")))
    }

    fn flush(&mut self) -> Result<(), OutputError> {
        std::io::Write::flush(&mut self.0)
            .map_err(|e| OutputError::new(alloc::format!("io: {e}")))
    }
}

/// Convenience: box a `NullSink` as the type-erased default
/// `Vm::stdout` value. Centralised so `Vm::new` and the
/// `Runtime::reset` paths agree once `Vm::stdout` migrates
/// to `Box<dyn OutputSink>` (Phase 1 of ADR 0018).
#[allow(dead_code)] // Used after Phase 1's vm.stdout migration.
pub fn null_sink() -> Box<dyn OutputSink> {
    Box::new(NullSink)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use alloc::string::ToString;

    /// Test-only sink that accumulates bytes for assertion.
    /// Mirrors the per-fixture `Sink` adapter in
    /// `vm/iter.rs:2574` — once OutputSink is migrated to
    /// `rubyrs-core`, this becomes the canonical test sink.
    struct CaptureSink {
        buf: Vec<u8>,
    }

    impl OutputSink for CaptureSink {
        fn write_bytes(&mut self, buf: &[u8]) -> Result<(), OutputError> {
            self.buf.extend_from_slice(buf);
            Ok(())
        }
    }

    #[test]
    fn null_sink_drops_writes() {
        let mut s = NullSink;
        s.write_bytes(b"discarded").unwrap();
        s.flush().unwrap();
    }

    #[test]
    fn capture_sink_round_trip() {
        let mut s = CaptureSink { buf: Vec::new() };
        s.write_bytes(b"hello, ").unwrap();
        s.write_bytes(b"world").unwrap();
        assert_eq!(s.buf, b"hello, world");
    }

    #[test]
    fn write_fmt_default_routes_through_write_bytes() {
        let mut s = CaptureSink { buf: Vec::new() };
        s.write_fmt(format_args!("answer = {}", 42)).unwrap();
        assert_eq!(String::from_utf8(s.buf).unwrap(), "answer = 42");
    }

    #[test]
    fn write_macro_works_on_box_dyn_output_sink() {
        // The shape `Vm::stdout` ultimately uses: a boxed
        // trait object. The `write!` macro expands to a
        // `.write_fmt(...)` call which auto-derefs through
        // the Box.
        let mut s: Box<dyn OutputSink> = Box::new(CaptureSink { buf: Vec::new() });
        let _ = core::write!(s, "x={}, y={}", 1, 2);
        // We can't extract the buf from Box<dyn>; this test
        // only verifies the API compiles. The
        // `write_fmt_default_routes_through_write_bytes`
        // test above proves behaviour.
    }

    #[cfg(feature = "std-sink")]
    #[test]
    fn std_sink_adapts_vec_io_write() {
        let buf: Vec<u8> = Vec::new();
        let mut adapter = StdSink(buf);
        adapter.write_bytes(b"via std::io::Write").unwrap();
        assert_eq!(adapter.0, b"via std::io::Write");
    }

    #[test]
    fn output_error_message_round_trip() {
        let e = OutputError::new("disk full");
        assert_eq!(e.message(), "disk full");
        assert_eq!(e.to_string(), "disk full");
    }
}
