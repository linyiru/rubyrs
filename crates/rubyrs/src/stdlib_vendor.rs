//! Embedded pure-Ruby implementations of selected stdlib modules.
//! Gated behind the `stdlib` Cargo feature per ADR 0017 row 125:
//! Tier 1 default build provides only the "feature-absent surface"
//! (constant exists, calls raise NoMethodError) for stdlib names
//! in the lenient stub whitelist. With `--features stdlib` the
//! same require path additionally evaluates the embedded source
//! below on the running Vm, supplying CRuby-compatible behaviour
//! for the subset modelled.
//!
//! Each entry pairs a `require '<name>'` string with a
//! `&'static str` Ruby source body that uses only Tier 1 built-ins
//! (no fs, no random) so the deterministic subset matches CRuby
//! byte-for-byte under `diff_cruby`.

/// Extra pure-Ruby source that runs *unconditionally* in the
/// lenient-stub branch (not gated behind the `stdlib` feature),
/// for stdlib names whose ecosystem consumers assume specific
/// constants/methods at module-load time.
///
/// Currently scoped to `uri`: Rack 3 / Sinatra 4 evaluate
///
/// ```text
/// URI_PARSER = defined?(::URI::RFC2396_PARSER) ?
///                ::URI::RFC2396_PARSER : ::URI::DEFAULT_PARSER
/// ```
///
/// at the top of `rack/utils.rb` — i.e. before any request
/// handling — so unless one of those constants is materialised
/// at `require "uri"` time the require itself raises NameError
/// and blocks every Sinatra/Rack app from loading. The shim
/// provides both constants pointing at a minimal RFC2396_Parser
/// object whose `escape` / `unescape` methods cover what Rack
/// actually calls (`Rack::Utils.escape_path`,
/// `Rack::Utils.unescape`). The full URI parser surface stays
/// behind `--features stdlib` per ADR 0017.
///
/// Distinct from `stdlib_vendor_source` because this body runs
/// for everyone (the Sinatra spike needs it in the default
/// build), whereas the latter is the opt-in fuller stdlib.
pub(crate) fn always_on_stub_extras(name: &str) -> Option<&'static str> {
    match name {
        "uri" | "uri/generic" | "uri/common" => {
            Some(include_str!("stdlib_vendor/uri_parser_shim.rb"))
        }
        _ => None,
    }
}

/// Pure-Ruby source for a stdlib name, or `None` if rubyrs has
/// no embedded implementation. Caller (require dispatch in
/// `kernel.rs`) parses + compiles + executes the source on the
/// current Vm exactly once per script — the existing
/// `loaded_stdlib_stubs` set guards re-execution.
#[cfg(feature = "stdlib")]
pub(crate) fn stdlib_vendor_source(name: &str) -> Option<&'static str> {
    match name {
        "pathname" => Some(include_str!("stdlib_vendor/pathname.rb")),
        "set" => Some(include_str!("stdlib_vendor/set.rb")),
        "stringio" => Some(include_str!("stdlib_vendor/stringio.rb")),
        "strscan" => Some(include_str!("stdlib_vendor/strscan.rb")),
        "json" => Some(include_str!("stdlib_vendor/json.rb")),
        // ActiveSupport-lite menu item 3 (ADR 0026 v2). All three
        // common require-paths users reach for (`active_support`,
        // `active_support/all`, `active_support/core_ext`) route
        // to the same canon — the real gem also funnels into one
        // load tree, so users don't observe a difference.
        "active_support"
        | "active_support/all"
        | "active_support/core_ext" => Some(include_str!("stdlib_vendor/active_support_lite.rb")),
        _ => None,
    }
}
