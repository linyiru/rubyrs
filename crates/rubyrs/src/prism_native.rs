//! ADR 0036 Slice 1 — expose Prism's serialize-parse to Ruby so RuboCop's `parser_prism`
//! engine runs on rubyrs WITHOUT the prism C extension (which rubyrs cannot `dlopen` — it is
//! a CRuby-ABI `.bundle`). The prism C library itself is ALREADY LINKED into rubyrs: the
//! `ruby-prism` crate uses it for rubyrs's own frontend, so `pm_serialize_parse*` and the
//! `pm_buffer_*` helpers are present in the binary. These host fns call them directly and
//! return the serialized blob; the prism gem's pure-Ruby `Prism::Serialize.load_parse(_lex)`
//! (well within rubyrs's executable subset — no heavy metaprogramming) inflates the blob into
//! a `Prism::Node` tree, which `Prism::Translation::Parser` turns into the `Parser::AST::Node`
//! tree RuboCop's cops consume. This deletes RuboCop's slow interpreted whitequark lexer
//! (rubyrs's worst-case shape, ~38x CRuby) — the dominant per-file cost.

use crate::error::{RubyError, Trap};
use crate::value::Value;

/// The pure-Ruby Prism backend rubyrs injects in place of the `prism/prism` C extension
/// (ADR 0036). Defines `Prism.parse`/`parse_lex` over the host fns below + the gem's
/// `Prism::Serialize`. The `require` handler runs this when "prism/prism" is required.
pub(crate) const BACKEND_RB: &str = include_str!("prism_native_backend.rb");

/// Mirror of `pm_buffer_t` (include/prism/util/pm_buffer.h:22 —
/// `{ size_t length; size_t capacity; char *value; }`). Used as an opaque owner: prism's
/// helpers manage the inner allocation; we only read `value`/`length` out and `free`.
#[repr(C)]
struct PmBuffer {
    length: usize,
    capacity: usize,
    value: *mut u8,
}

// The prism C library — already linked via the `ruby-prism` dependency (unconditional).
unsafe extern "C" {
    fn pm_buffer_init(buffer: *mut PmBuffer) -> bool;
    fn pm_buffer_value(buffer: *const PmBuffer) -> *const u8;
    fn pm_buffer_length(buffer: *const PmBuffer) -> usize;
    fn pm_buffer_free(buffer: *mut PmBuffer);
    fn pm_serialize_parse(buffer: *mut PmBuffer, source: *const u8, size: usize, data: *const u8);
    fn pm_serialize_parse_lex(buffer: *mut PmBuffer, source: *const u8, size: usize, data: *const u8);
}

/// Serialize a parse (or parse+lex) of `src` to the prism wire format. `opts` is prism's
/// serialized options blob (the `pm_options` wire format, built by the backend's `dump_options`
/// — filepath/version/partial_script/encoding that RuboCop's `Prism::Translation::Parser`
/// passes). An empty/absent blob passes `data == NULL`, selecting prism's defaults
/// (the documented NULL-options call, prism.h:378).
fn serialize(src: &[u8], opts: Option<&[u8]>, lex: bool) -> Vec<u8> {
    // SAFETY: `pm_buffer_init` initialises the owner; `pm_serialize_parse*` append the blob
    // into it; we copy the bytes out before `pm_buffer_free` releases them. `data` (when set)
    // borrows `opts` for the duration of the call only. The buffer never escapes this call.
    // Single-threaded host-fn scope.
    unsafe {
        let mut buf = PmBuffer { length: 0, capacity: 0, value: std::ptr::null_mut() };
        if !pm_buffer_init(&mut buf) {
            return Vec::new();
        }
        let data = match opts {
            Some(o) if !o.is_empty() => o.as_ptr(),
            _ => std::ptr::null(),
        };
        if lex {
            pm_serialize_parse_lex(&mut buf, src.as_ptr(), src.len(), data);
        } else {
            pm_serialize_parse(&mut buf, src.as_ptr(), src.len(), data);
        }
        let vptr = pm_buffer_value(&buf);
        let vlen = pm_buffer_length(&buf);
        let blob = if vptr.is_null() || vlen == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(vptr, vlen).to_vec()
        };
        pm_buffer_free(&mut buf);
        blob
    }
}

/// `(source)` or `(source, options_blob)` — the optional second arg is the serialized
/// `pm_options` blob (or `nil` for defaults).
fn arg_src(args: &[Value], sig: &str) -> Result<(Vec<u8>, Option<Vec<u8>>), Trap> {
    match args {
        [Value::Str(s)] | [Value::Str(s), Value::Nil] => Ok((s.content.borrow().clone(), None)),
        [Value::Str(s), Value::Str(o)] => {
            Ok((s.content.borrow().clone(), Some(o.content.borrow().clone())))
        }
        _ => Err(Trap {
            err: RubyError::ArgumentError { msg: sig.to_string() },
            backtrace: vec![],
        }),
    }
}

/// Register `__rubyrs_prism_serialize_parse` / `__rubyrs_prism_serialize_parse_lex` on `rt`.
/// The rubyrs prism shim (`prism/prism` replacement on `$LOAD_PATH`) detects these and
/// builds `Prism.parse` / `Prism.parse_lex` on top of them.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    rt.register_fn("__rubyrs_prism_serialize_parse", |args| {
        let (src, opts) = arg_src(args, "__rubyrs_prism_serialize_parse(source: String, options: String = nil)")?;
        Ok(Value::new_str_bytes_binary(serialize(&src, opts.as_deref(), false)))
    });
    rt.register_fn("__rubyrs_prism_serialize_parse_lex", |args| {
        let (src, opts) = arg_src(args, "__rubyrs_prism_serialize_parse_lex(source: String, options: String = nil)")?;
        Ok(Value::new_str_bytes_binary(serialize(&src, opts.as_deref(), true)))
    });
}
