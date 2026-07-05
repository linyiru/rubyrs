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

/// No-op — the `_oj` battery needs no registration. Kept so the CLI's
/// uniform per-battery `register_*_host_fns` sequence (and any
/// embedder following it) stays stable, and as the slot should the
/// battery ever grow real host fns.
///
/// This USED to eval `require "json"` (the `Oj` bodies reference
/// `JSON`), but an eager require put "json" in loaded_features before
/// any user code ran, so a user script's first `require "json"`
/// returned false where CRuby returns true — real oj is a C extension
/// with its own parser and never loads stdlib json (probed against
/// oj 3.17.0; caught by the stdlib_require_stub diff fixture under
/// `--features stdlib,_oj`). The require is now lazy at first Oj
/// method use (`Oj.__ensure_json` in preamble/oj_ext.rb), which also
/// keeps it out of the cached preamble chunks (a require inside one
/// would re-parse the vendored json.rb on every cache replay).
pub fn register_host_fns(_rt: &mut crate::Runtime) {}
