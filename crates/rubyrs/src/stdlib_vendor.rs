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

/// Pure-Ruby source for a stdlib name, or `None` if rubyrs has
/// no embedded implementation. Caller (require dispatch in
/// `kernel.rs`) parses + compiles + executes the source on the
/// current Vm exactly once per script — the existing
/// `loaded_stdlib_stubs` set guards re-execution.
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
