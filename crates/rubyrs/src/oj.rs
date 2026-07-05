//! `_oj` battery — a pure-Ruby stand-in for the oj gem's `oj/oj` C
//! extension. `Oj.dump` / `Oj.load` route to rubyrs's JSON (oj's
//! `:compat` mode == standard JSON), which is correct for the common
//! "fast JSON drop-in" usage (dumping/loading plain data). oj's default
//! `:object`-mode quirks — symbols as `":sym"`, custom-object
//! marshalling, circular refs — are intentionally NOT modelled (see the
//! preamble). No host fns; the `Oj` module (preamble/oj_ext.rb) is
//! loaded by `load_preamble_inner` at Runtime construction through the
//! preamble bytecode cache, and `require "oj/oj"` is wired to succeed
//! in the require path.

/// `require "json"` for the `Oj` module (no host fns — the shim is pure
/// Ruby over JSON). The module definition itself lives in the cached
/// preamble pipeline; the require stays HERE (registration time)
/// because a `require` inside a cached preamble chunk would re-parse
/// the vendored json.rb on every cache replay — `JSON` is only
/// referenced from Oj method bodies, so requiring it after the module
/// definition is equivalent. Mirrors the socket/bcrypt battery
/// registration shape so the CLI registers it the same way.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    if let Err(trap) = rt.eval("require \"json\"", "<rubyrs:oj_ext:require-json>") {
        panic!("ICE: _oj failed to require json: {trap:?}");
    }
}
