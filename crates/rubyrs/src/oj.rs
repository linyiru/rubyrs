//! `_oj` battery — a pure-Ruby stand-in for the oj gem's `oj/oj` C
//! extension. `Oj.dump` / `Oj.load` route to rubyrs's JSON (oj's
//! `:compat` mode == standard JSON), which is correct for the common
//! "fast JSON drop-in" usage (dumping/loading plain data). oj's default
//! `:object`-mode quirks — symbols as `":sym"`, custom-object
//! marshalling, circular refs — are intentionally NOT modelled (see the
//! preamble). No host fns; the battery only evals the preamble (which
//! defines the `Oj` module) at startup, and `require "oj/oj"` is wired
//! to succeed in the require path.

/// Eval the `Oj` preamble (no host fns — the shim is pure Ruby over
/// JSON). Mirrors the socket/bcrypt battery shape so the CLI registers
/// it the same way.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    const PREAMBLE: &str = include_str!("preamble/oj_ext.rb");
    if let Err(trap) = rt.eval(PREAMBLE, "<rubyrs:oj_ext>") {
        panic!("ICE: _oj failed to load preamble: {trap:?}");
    }
}
