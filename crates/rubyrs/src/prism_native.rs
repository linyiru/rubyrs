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
    fn pm_serialize_lex(buffer: *mut PmBuffer, source: *const u8, size: usize, data: *const u8);
}

/// Which `pm_serialize_*` entry point to call — each emits a DIFFERENT wire layout, matched
/// by a different `Prism::Serialize.load_*` on the Ruby side (`load_parse` / `load_parse_lex`
/// / `load_lex` respectively).
#[derive(Copy, Clone)]
enum SerializeMode {
    Parse,
    ParseLex,
    Lex,
}

/// Serialize a parse (or parse+lex, or lex-only) of `src` to the prism wire format. `opts` is
/// prism's serialized options blob (the `pm_options` wire format, built by the backend's
/// `dump_options` — filepath/version/partial_script/encoding that RuboCop's
/// `Prism::Translation::Parser` passes). An empty/absent blob passes `data == NULL`, selecting
/// prism's defaults (the documented NULL-options call, prism.h:378).
fn serialize(src: &[u8], opts: Option<&[u8]>, mode: SerializeMode) -> Vec<u8> {
    // SAFETY: `pm_buffer_init` initialises the owner; `pm_serialize_*` append the blob
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
        match mode {
            SerializeMode::Parse => pm_serialize_parse(&mut buf, src.as_ptr(), src.len(), data),
            SerializeMode::ParseLex => pm_serialize_parse_lex(&mut buf, src.as_ptr(), src.len(), data),
            SerializeMode::Lex => pm_serialize_lex(&mut buf, src.as_ptr(), src.len(), data),
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

/// `(source, options_blob_or_nil)` for the materializing entry points — the source STRING
/// VALUE itself is needed (the resulting `Prism::Source` wraps it / a dup of it), not just
/// its bytes.
fn arg_src_value(args: &[Value], sig: &str) -> Result<(std::rc::Rc<crate::value::RStr>, Option<Vec<u8>>), Trap> {
    match args {
        [Value::Str(s)] | [Value::Str(s), Value::Nil] => Ok((s.clone(), None)),
        [Value::Str(s), Value::Str(o)] => Ok((s.clone(), Some(o.content.borrow().clone()))),
        _ => Err(Trap {
            err: RubyError::ArgumentError { msg: sig.to_string() },
            backtrace: vec![],
        }),
    }
}

/// Register the prism host fns on `rt`. The rubyrs prism shim (`prism/prism` replacement
/// on `$LOAD_PATH`) detects these and builds `Prism.parse` / `Prism.parse_lex` /
/// `Prism.lex` on top:
///
/// - `__rubyrs_prism_serialize_parse{,_lex}` / `__rubyrs_prism_serialize_lex` — the wire
///   blob, for the interpreted `Prism::Serialize` deserializer (ADR 0036 Slice 1; now the
///   fallback path for parse/parse_lex, the primary path for lex).
/// - `__rubyrs_prism_materialize_parse{,_lex}` — parse AND build the
///   `Prism::ParseResult` / `ParseLexResult` object graph natively (Slice 2), skipping
///   the interpreted deserializer entirely. Returns `nil` to decline (version/encoding/
///   class mismatch), in which case the backend falls back to the Serialize path.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    rt.register_fn("__rubyrs_prism_serialize_parse", |args| {
        let (src, opts) = arg_src(args, "__rubyrs_prism_serialize_parse(source: String, options: String = nil)")?;
        Ok(Value::new_str_bytes_binary(serialize(&src, opts.as_deref(), SerializeMode::Parse)))
    });
    rt.register_fn("__rubyrs_prism_serialize_parse_lex", |args| {
        let (src, opts) = arg_src(args, "__rubyrs_prism_serialize_parse_lex(source: String, options: String = nil)")?;
        Ok(Value::new_str_bytes_binary(serialize(&src, opts.as_deref(), SerializeMode::ParseLex)))
    });
    rt.register_fn("__rubyrs_prism_serialize_lex", |args| {
        let (src, opts) = arg_src(args, "__rubyrs_prism_serialize_lex(source: String, options: String = nil)")?;
        Ok(Value::new_str_bytes_binary(serialize(&src, opts.as_deref(), SerializeMode::Lex)))
    });
    rt.register_fn("__rubyrs_prism_materialize_parse", |args| {
        let (src, opts) = arg_src_value(args, "__rubyrs_prism_materialize_parse(source: String, options: String = nil)")?;
        let ptr = materialize_vm_ptr()?;
        // SAFETY: see json_native.rs — the pointer is installed by the dispatch site for
        // this call's synchronous duration; the borrow is not stashed.
        let vm = unsafe { &mut *ptr };
        let blob = serialize(&src.content.borrow().clone(), opts.as_deref(), SerializeMode::Parse);
        Ok(crate::prism_materialize::materialize_parse(vm, &src, &blob).unwrap_or(Value::Nil))
    });
    rt.register_fn("__rubyrs_prism_materialize_parse_lex", |args| {
        let (src, opts) = arg_src_value(args, "__rubyrs_prism_materialize_parse_lex(source: String, options: String = nil)")?;
        let ptr = materialize_vm_ptr()?;
        // SAFETY: as above.
        let vm = unsafe { &mut *ptr };
        let blob = serialize(&src.content.borrow().clone(), opts.as_deref(), SerializeMode::ParseLex);
        Ok(crate::prism_materialize::materialize_parse_lex(vm, &src, &blob).unwrap_or(Value::Nil))
    });
}

/// The current VM pointer, for host fns that materialize heap objects (same pattern as
/// json_native.rs). Errors when called outside host-fn scope.
fn materialize_vm_ptr() -> Result<*mut crate::vm::Vm, Trap> {
    let ptr = crate::vm::current_vm_ptr();
    if ptr.is_null() {
        return Err(Trap {
            err: RubyError::RuntimeError {
                msg: "prism_native: CURRENT_VM_PTR null — called outside host-fn scope".to_string(),
            },
            backtrace: vec![],
        });
    }
    Ok(ptr)
}
