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
///
/// `cfg(not(target_os = "wasi"))` mirrors the caller's gating in
/// `vm/kernel.rs` — the `require` resolution arm that calls this
/// helper is itself wasi-excluded (no filesystem on wasm32-wasip1
/// builds), so without the gate the wasm `--no-default-features`
/// build trips `-D dead-code`.
#[cfg(not(target_os = "wasi"))]
pub(crate) fn always_on_stub_extras(name: &str) -> Option<&'static str> {
    match name {
        "uri" | "uri/generic" | "uri/common" => {
            Some(include_str!("stdlib_vendor/uri_parser_shim.rb"))
        }
        // `tilt`: Sinatra 4 `require`s tilt at module-load time but
        // only calls `Tilt.default_mapping.extensions_for(engine)`
        // from inside the view-rendering path. A minimal shim with
        // an `EmptyMapping` keeps `require "tilt"` succeeding so
        // Sinatra-on-rubyrs reaches a route handler; real template
        // rendering remains absent (ADR 0017 feature-absent surface).
        "tilt" => {
            Some(include_str!("stdlib_vendor/tilt_shim.rb"))
        }
        // `forwardable`: Sinatra / Mustermann / Rack call
        // `extend Forwardable` + `def_delegators :recv, *methods`
        // from class bodies (executed at module-load time). The
        // kernel-side stub already installs empty Forwardable +
        // SingleForwardable shells; this shim reopens both and
        // installs the actual delegation surface so `require
        // "forwardable"` is functional, not just resolvable.
        "forwardable" => {
            Some(include_str!("stdlib_vendor/forwardable_shim.rb"))
        }
        // `delegate`: Mustermann's
        // `class NodeTranslator < DelegateClass(Node)` shape
        // (mustermann/ast/translator.rb:18). The kernel stub
        // creates empty Delegator + SimpleDelegator shells; this
        // shim fills them with method_missing-based forwarding
        // and installs the top-level `DelegateClass(superclass)`
        // factory so subclassing succeeds at module-load time.
        "delegate" => {
            Some(include_str!("stdlib_vendor/delegate_shim.rb"))
        }
        // `yaml` / `safe_yaml`: a focused pure-Ruby YAML loader for
        // the front-matter / config subset. `YAML.load` /
        // `SafeYAML.load` parse directly; safe_yaml's real
        // Psych::Handler internals (which rubyrs can't satisfy) are
        // bypassed. Discovery: P3 Jekyll spike — jekyll reads
        // front-matter via SafeYAML.load / load_file.
        "yaml" | "safe_yaml" | "safe_yaml/load" => {
            Some(include_str!("stdlib_vendor/yaml.rb"))
        }
        // `logger`: reopen the Logger shell with the severity-level
        // constants + the debug/info/warn/error/add surface (and the
        // format_* helpers subclasses call). Discovery: P3 Jekyll
        // spike — jekyll's Stevenson < Logger writer.
        "logger" => {
            Some(include_str!("stdlib_vendor/logger.rb"))
        }
        // `jekyll-sass-converter`: the real gem requires sass-embedded
        // (native dart-sass). The shim defines the Jekyll converter
        // classes and routes SCSS→CSS to the `RubyrsSass.compile` host
        // primitive (grass-backed `sass` battery). See
        // `is_blessed_reimpl_name`.
        "jekyll-sass-converter" => {
            Some(include_str!("stdlib_vendor/jekyll_sass_converter_shim.rb"))
        }
        // A Rational-backed BigDecimal + the Kernel#BigDecimal()
        // conversion function. Always-on (not stdlib-gated) so default-
        // build code that `require "bigdecimal"` — e.g. liquid's numeric
        // filters via `Utils.to_number` — gets a working decimal type.
        "bigdecimal" => Some(include_str!("stdlib_vendor/bigdecimal.rb")),
        // `cgi`: the escape/unescape surface liquid's standard filters
        // call (escape → CGI.escapeHTML, url_encode → CGI.escape).
        // Always-on for the same reason as bigdecimal: liquid is a
        // default-build consumer.
        "cgi" | "cgi/util" | "cgi/escape" => Some(include_str!("stdlib_vendor/cgi.rb")),
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
        // `etc` / `monitor`: tiny single-threaded-model subsets —
        // see each file's header for the divergence notes.
        // Discovery: minitest 5.25 requires "etc" unconditionally;
        // logger 1.7's LogDevice requires "monitor".
        "etc" => Some(include_str!("stdlib_vendor/etc.rb")),
        "timeout" => Some(include_str!("stdlib_vendor/timeout.rb")),
        "monitor" => Some(include_str!("stdlib_vendor/monitor.rb")),
        // OptionParser: declarative-on + parse! subset (minitest's
        // process_args). Replaces the old lenient shell, which
        // accepted the DSL but parsed nothing — minitest then
        // appended a fallback "--seed 0" and ignored every filter.
        "optparse" => Some(include_str!("stdlib_vendor/optparse.rb")),
        "set" => Some(include_str!("stdlib_vendor/set.rb")),
        "stringio" => Some(include_str!("stdlib_vendor/stringio.rb")),
        "strscan" => Some(include_str!("stdlib_vendor/strscan.rb")),
        "json" => Some(include_str!("stdlib_vendor/json.rb")),
        // `digest` (+ the per-algorithm require paths): a pure-Ruby
        // veneer defining `Digest::SHA2 / SHA256 / SHA1 / MD5` over
        // the native `RubyrsDigest` host primitive. The real `digest`
        // is a C extension (OpenSSL-backed); this is the ADR 0026
        // blessed reimpl. Discovery: P3 Jekyll spike —
        // `jekyll/cache.rb` keys its disk cache with
        // `Digest::SHA2.hexdigest(key)`.
        "digest" | "digest/sha2" | "digest/sha1" | "digest/md5" => {
            Some(include_str!("stdlib_vendor/digest.rb"))
        }
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
