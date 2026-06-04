//! Lock the URI parser shim that the lenient `require "uri"` path
//! installs (kernel.rs → `always_on_stub_extras` → vendored
//! `uri_parser_shim.rb`). Sinatra 4 / Rack 3 evaluate
//!
//!     URI_PARSER = defined?(::URI::RFC2396_PARSER) ?
//!                    ::URI::RFC2396_PARSER : ::URI::DEFAULT_PARSER
//!
//! at the top of `rack/utils.rb` — i.e. while requiring the gem,
//! before any request handling. The shim must:
//!   (1) define both constants so that line resolves,
//!   (2) have them point at one shared instance so identity
//!       checks in downstream code line up, and
//!   (3) expose `escape` / `unescape` with CRuby-compatible
//!       byte-level semantics — including a clean UTF-8
//!       roundtrip. The shim's `unescape` uses the canonical
//!       CRuby idiom `gsub(/%XX/) { [hex].pack('C') }`, which
//!       relies on the VM's `String#gsub` block-result path
//!       preserving raw bytes from a `Value::Str` return; that
//!       was a regression earlier in this PR (an inline
//!       byte-array workaround sidestepped it) and is now fixed
//!       at the VM layer (`vm/iter.rs`). This test file locks
//!       BOTH halves: the shim contract AND the underlying
//!       byte-fidelity guarantee, so a regression in either
//!       surface re-breaks Sinatra-on-rubyrs.

use std::path::PathBuf;
use std::process::Command;
use std::fs;

fn run(script: &str, name: &str) -> String {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = tmp.join(name);
    fs::write(&path, script).unwrap();
    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&path)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "rubyrs exited non-zero.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr,
    );
    stdout
}

#[test]
fn require_uri_defines_default_parser_and_rfc2396_parser_aliased() {
    // Pins (1) + (2) above: both constants exist after
    // `require "uri"` and point at the same instance — the
    // identity check is what Rack-style code uses implicitly.
    let out = run(
r#"
require "uri"
puts "default-defined=#{defined?(URI::DEFAULT_PARSER)}"
puts "rfc2396-defined=#{defined?(URI::RFC2396_PARSER)}"
puts "same-instance=#{URI::DEFAULT_PARSER.equal?(URI::RFC2396_PARSER)}"
"#,
        "uri_shim_constants_driver.rb",
    );
    assert_eq!(
        out,
        "default-defined=constant\n\
         rfc2396-defined=constant\n\
         same-instance=true\n",
        "URI parser constants missing or split — Sinatra/Rack \
         load-time `URI_PARSER = ... ? RFC2396_PARSER : DEFAULT_PARSER` \
         expression will not resolve.\nstdout:\n{}",
        out,
    );
}

#[test]
fn uri_default_parser_escape_unescape_ascii() {
    // Pins (3) for the ASCII case Rack hits most often
    // (`Rack::Utils.escape_path` / `Rack::Utils.unescape`). Reserved
    // chars stay literal; unsafe chars (space, here) become `%XX`.
    let out = run(
r#"
require "uri"
p = URI::DEFAULT_PARSER
puts "escape: #{p.escape('foo bar baz')}"
puts "unescape: #{p.unescape('foo%20bar%2Fbaz')}"
puts "roundtrip-empty: #{p.unescape(p.escape('')).inspect}"
"#,
        "uri_shim_ascii_driver.rb",
    );
    assert_eq!(
        out,
        "escape: foo%20bar%20baz\n\
         unescape: foo bar/baz\n\
         roundtrip-empty: \"\"\n",
        "URI parser ASCII escape/unescape diverged.\nstdout:\n{}",
        out,
    );
}

#[test]
fn uri_default_parser_unescape_preserves_utf8_bytes() {
    // Pins (3) for the UTF-8 case — the contract that broke
    // Sinatra-on-rubyrs at first probe and now must stay nailed
    // down.
    //
    // Historical regression: before the VM fix in this PR,
    // `String#gsub(/%XX/) { |_| [byte].pack('C') }` (the natural
    // CRuby idiom for `unescape`) lossy-decoded each raw byte
    // returned from the block to `U+FFFD` (3 bytes), so
    // `%E4%B8%AD` (中) round-tripped as `���` instead of `中`. The
    // shim originally carried a byte-array + `pack('C*')`
    // workaround; once the VM splice path was fixed to copy
    // `Value::Str` raw bytes through unchanged, the shim reverted
    // to the canonical CRuby shape — this test continues to lock
    // BOTH halves end-to-end, so a future "simplify the VM gsub
    // back to lossy" or "rewrite unescape" change that re-breaks
    // multi-byte URI decoding gets caught here.
    let out = run(
r#"
require "uri"
p = URI::DEFAULT_PARSER
original = "hello world 中文"
escaped = p.escape(original)
unescaped = p.unescape(escaped)
puts "escaped-bytes: #{escaped.bytes.inspect}"
puts "roundtrip-equal: #{unescaped == original}"
puts "roundtrip-bytes: #{unescaped.bytes.inspect}"
"#,
        "uri_shim_utf8_driver.rb",
    );
    let expected = "\
escaped-bytes: [104, 101, 108, 108, 111, 37, 50, 48, 119, 111, 114, 108, 100, \
37, 50, 48, 37, 69, 52, 37, 66, 56, 37, 65, 68, 37, 69, 54, 37, 57, 54, 37, 56, 55]
roundtrip-equal: true
roundtrip-bytes: [104, 101, 108, 108, 111, 32, 119, 111, 114, 108, 100, 32, \
228, 184, 173, 230, 150, 135]
";
    assert_eq!(
        out, expected,
        "UTF-8 URI roundtrip diverged — the gsub-binary-byte \
         workaround in the shim's `unescape` may have regressed.\n\
         stdout:\n{}",
        out,
    );
}

#[test]
fn uri_default_parser_escape_accepts_custom_unsafe_regexp() {
    // Rack passes a custom `unsafe` regexp to
    // `URI_PARSER.escape` (e.g. `Rack::Utils::PATH_UNSAFE`), so
    // the 2-arg form must be supported. Without it, the default
    // unsafe set would escape too few characters and routes
    // containing path-significant chars would slip through.
    let out = run(
r#"
require "uri"
# Custom unsafe set: only space gets encoded.
custom = /[ ]/
puts "custom: #{URI::DEFAULT_PARSER.escape('foo/bar baz?x=1', custom)}"
"#,
        "uri_shim_custom_unsafe_driver.rb",
    );
    assert_eq!(
        out,
        "custom: foo/bar%20baz?x=1\n",
        "URI parser custom-unsafe-regexp form broken.\nstdout:\n{}",
        out,
    );
}

#[test]
fn require_uri_subpaths_route_to_same_shim() {
    // CRuby's `uri` library installs the parser under each of
    // `uri`, `uri/common`, and `uri/generic` (different files,
    // same constants). The lenient stub treats all three as
    // synonyms (kernel.rs:2913 + `always_on_stub_extras`), so a
    // gem that requires a subpath gets the same parser. Pin
    // that — a future split of the routing table that forgets a
    // subpath would silently break consumers like webrick,
    // mechanize, etc.
    //
    // Critical extra contract: the parser INSTANCE must survive
    // across the two requires unchanged. `loaded_stdlib_stubs`
    // dedups per raw require path, so `require "uri"` followed
    // by `require "uri/common"` re-enters the lenient-stub
    // branch with a fresh path key and would re-evaluate the
    // shim — replacing `URI::DEFAULT_PARSER` with a new instance
    // and silently invalidating any memoized reference (e.g.
    // `URI_PARSER = ::URI::RFC2396_PARSER` at the top of
    // rack/utils.rb). The shim's `unless defined?(...)` guard
    // is what keeps the instance stable; this test pins that
    // guarantee with an `equal?` identity assertion (NOT just
    // `defined?`).
    let out = run(
r#"
require "uri"
first_default = URI::DEFAULT_PARSER
first_rfc = URI::RFC2396_PARSER

require "uri/common"
puts "after-common-default-eq: #{first_default.equal?(URI::DEFAULT_PARSER)}"
puts "after-common-rfc-eq: #{first_rfc.equal?(URI::RFC2396_PARSER)}"

require "uri/generic"
puts "after-generic-default-eq: #{first_default.equal?(URI::DEFAULT_PARSER)}"
puts "after-generic-rfc-eq: #{first_rfc.equal?(URI::RFC2396_PARSER)}"
"#,
        "uri_shim_subpaths_driver.rb",
    );
    assert_eq!(
        out,
        "after-common-default-eq: true\n\
         after-common-rfc-eq: true\n\
         after-generic-default-eq: true\n\
         after-generic-rfc-eq: true\n",
        "URI parser instance changed across subpath requires — \
         `URI_PARSER = ::URI::RFC2396_PARSER` memoization in \
         rack/utils.rb would silently diverge from later \
         `URI::DEFAULT_PARSER` lookups.\nstdout:\n{}",
        out,
    );
}
